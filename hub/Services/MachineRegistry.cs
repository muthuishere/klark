using System.Collections.Concurrent;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace KlarkHub.Services;

/// <summary>
/// Every klark machine currently linked to this hub. One instance for the whole
/// server, shared by all signed-in users: that is what lets many people gather
/// on one hub and see every machine's panes.
/// </summary>
public sealed class MachineRegistry : IDisposable
{
    private readonly ConcurrentDictionary<string, MachineConnection> _machines = new();
    private readonly ConcurrentDictionary<string, MachineConnection> _sessions = new();
    private readonly ILogger<MachineRegistry> _log;
    private readonly HubStore _store;
    private readonly Timer _sweeper;
    private readonly object _openLock = new();

    public MachineRegistry(ILogger<MachineRegistry> log, HubStore store)
    {
        _log = log;
        _store = store;
        _sweeper = new Timer(_ => Tick(), null, TimeSpan.FromSeconds(5), TimeSpan.FromSeconds(5));
    }

    /// <summary>A machine is offline once it has had no event stream open for this long (ADR 0013).</summary>
    public TimeSpan OfflineAfter { get; set; } = TimeSpan.FromSeconds(45);

    /// <summary>How often an idle event stream carries a heartbeat line.</summary>
    public TimeSpan Heartbeat
    {
        get => _heartbeat;
        set
        {
            _heartbeat = value;
            var period = value < TimeSpan.FromSeconds(5) ? value : TimeSpan.FromSeconds(5);
            _sweeper.Change(period, period);
        }
    }
    private TimeSpan _heartbeat = TimeSpan.FromSeconds(15);

    /// <summary>
    /// The one hub-wide timer: drops offline machines and rings each quiet stream for its
    /// heartbeat line. Streams themselves own no timers; each is a single awaiting writer.
    /// </summary>
    private void Tick()
    {
        Sweep();
        foreach (var m in _sessions.Values) m.PingIfQuiet(_heartbeat);
    }

    public void Dispose() => _sweeper.Dispose();

    internal HubStore Store => _store;

    /// <summary>Raised when a machine connects, disconnects, or sends a new tree.</summary>
    public event Action? Changed;

    public IReadOnlyCollection<MachineConnection> Machines =>
        _machines.Values.OrderBy(m => m.Name, StringComparer.OrdinalIgnoreCase).ToList();

    public MachineConnection? Get(string id) => _machines.TryGetValue(id, out var m) ? m : null;

    /// <summary>Pull every connected machine's tree that is older than maxAge, in parallel.</summary>
    public Task RefreshTreesAsync(TimeSpan maxAge, Func<MachineConnection, bool>? only = null) =>
        Task.WhenAll(_machines.Values.Where(m => only is null || only(m)).Select(async m =>
        {
            try { await m.EnsureTreeAsync(maxAge); }
            catch (Exception e) when (e is TimeoutException or InvalidOperationException) { }
        }));

    internal void NotifyChanged()
    {
        try { Changed?.Invoke(); }
        catch (Exception e) { _log.LogWarning(e, "change handler failed"); }
    }

    /// <summary>Close the live links of a machine name (and its fleet members, "name:*").</summary>
    public Task DisconnectAsync(string name)
    {
        foreach (var m in _machines.Values.Where(m => string.Equals(m.Id, name, StringComparison.OrdinalIgnoreCase) ||
                                                     m.Id.StartsWith(name + ":", StringComparison.OrdinalIgnoreCase)).ToList())
        {
            Remove(m, "revoked");
        }
        return Task.CompletedTask;
    }

    /// <summary>
    /// Open a link session for an authenticated machine (POST /agent/session, ADR 0013).
    /// A new session for a machine that is already linked replaces the old one: that is
    /// how a reconnect looks. Returns null with an error for a hello this hub cannot serve.
    /// </summary>
    public (MachineConnection? Conn, string? Error) Open(string machineId, string token, JsonNode? hello)
    {
        if (hello?["type"]?.GetValue<string>() != "hello") return (null, "the body must be a hello message");
        var protocol = hello["protocol"]?.GetValue<int>() ?? 0;
        if (protocol is < MachineConnection.MinProtocol or > MachineConnection.MaxProtocol)
        {
            // A machine on another protocol would misread what we send (STATUS S-10).
            _log.LogWarning("machine {Machine} speaks protocol {Protocol}; this hub speaks {Min}-{Max}", machineId, protocol,
                MachineConnection.MinProtocol, MachineConnection.MaxProtocol);
            return (null, $"unsupported protocol {protocol}; hub speaks {MachineConnection.MinProtocol}-{MachineConnection.MaxProtocol}");
        }
        var conn = new MachineConnection(machineId, TokenHash.Sha256Hex(token), this, _log);
        lock (_openLock)
        {
            if (_machines.TryGetValue(conn.Id, out var previous))
            {
                _log.LogInformation("machine {Machine} reconnected; dropping the old session", conn.Id);
                Remove(previous, "replaced");
            }
            _machines[conn.Id] = conn;
            _sessions[conn.SessionId] = conn;
        }
        conn.Hello(hello);
        return (conn, null);
    }

    /// <summary>The live session with this id whose machine token is <paramref name="token"/>; null otherwise.</summary>
    public MachineConnection? Session(string? sessionId, string token, out bool wrongToken)
    {
        wrongToken = false;
        if (string.IsNullOrEmpty(sessionId) || !_sessions.TryGetValue(sessionId, out var conn)) return null;
        if (!conn.TokenMatches(token))
        {
            wrongToken = true;
            return null;
        }
        return conn;
    }

    /// <summary>Drop machines whose event stream has been gone for <see cref="OfflineAfter"/>.</summary>
    internal void Sweep()
    {
        foreach (var m in _sessions.Values.Where(m => m.IdleFor() > OfflineAfter).ToList())
        {
            _log.LogInformation("machine {Machine} offline: no event stream for {Seconds:0}s", m.Id, m.IdleFor().TotalSeconds);
            Remove(m, "offline");
        }
    }

    private void Remove(MachineConnection conn, string why)
    {
        if (!_sessions.TryRemove(new KeyValuePair<string, MachineConnection>(conn.SessionId, conn))) return;
        _machines.TryRemove(new KeyValuePair<string, MachineConnection>(conn.Id, conn));
        conn.Close(why);
        conn.FailPending();
        _store.MachineDisconnected(conn.Id);
        NotifyChanged();
    }
}

/// <summary>One linked klark machine.</summary>
public sealed class MachineConnection
{
    private readonly MachineRegistry _registry;
    private readonly ILogger _log;
    private readonly byte[] _tokenSha;
    private readonly CancellationTokenSource _closed = new();
    // Outbound: hub messages with their seq, kept until the machine's stream
    // asks for something after them (resume after a dropped stream).
    private readonly object _outLock = new();
    private readonly List<(long Seq, byte[] Line)> _outbox = new();
    private long _outSeq;
    // The stream's doorbell: rung when a message is queued or a heartbeat is due.
    private readonly System.Threading.Channels.Channel<bool> _bell =
        System.Threading.Channels.Channel.CreateBounded<bool>(new System.Threading.Channels.BoundedChannelOptions(1)
        {
            FullMode = System.Threading.Channels.BoundedChannelFullMode.DropWrite,
            SingleReader = true,
        });
    private int _pingDue;
    private long _lastWriteTicks = DateTimeOffset.UtcNow.UtcTicks;
    private static readonly byte[] PingLine = "{\"type\":\"ping\"}\n"u8.ToArray();
    private CancellationTokenSource? _stream;
    private int _streams;
    private long _lastSeenTicks = DateTimeOffset.UtcNow.UtcTicks;
    private readonly SemaphoreSlim _pushLock = new(1, 1);
    private long _lastPush;
    private const int OutboxLimit = 4096;
    private readonly object _subsLock = new();
    private readonly Dictionary<string, List<PaneViewer>> _viewers = new();
    private readonly ConcurrentDictionary<string, TaskCompletionSource<JsonNode?>> _pending = new();
    private long _nextCallId;

    internal MachineConnection(string id, string tokenSha256, MachineRegistry registry, ILogger log)
    {
        Id = id;
        Name = id;
        SessionId = Convert.ToHexString(RandomNumberGenerator.GetBytes(16)).ToLowerInvariant();
        _tokenSha = Encoding.ASCII.GetBytes(tokenSha256);
        _registry = registry;
        _log = log;
    }

    /// <summary>The link session (ADR 0013); a reconnect gets a new one.</summary>
    public string SessionId { get; }

    internal bool TokenMatches(string token) =>
        CryptographicOperations.FixedTimeEquals(_tokenSha, Encoding.ASCII.GetBytes(TokenHash.Sha256Hex(token)));

    /// <summary>Whether the hub dropped this session (replaced, revoked, offline).</summary>
    public bool IsClosed => _closed.IsCancellationRequested;

    /// <summary>Zero while an event stream is open; otherwise how long since the machine was last heard.</summary>
    internal TimeSpan IdleFor() => Volatile.Read(ref _streams) > 0
        ? TimeSpan.Zero
        : DateTimeOffset.UtcNow - new DateTimeOffset(Interlocked.Read(ref _lastSeenTicks), TimeSpan.Zero);

    private void Seen() => Interlocked.Exchange(ref _lastSeenTicks, DateTimeOffset.UtcNow.UtcTicks);

    /// <summary>
    /// Serve GET /agent/events: NDJSON, one hub message per line with its seq, held open,
    /// a {"type":"ping"} line when idle. A newer stream for this session ends this one.
    /// </summary>
    public async Task StreamAsync(HttpResponse response, long after, CancellationToken ct)
    {
        var mine = CancellationTokenSource.CreateLinkedTokenSource(ct, _closed.Token);
        CancellationTokenSource? older;
        lock (_outLock)
        {
            older = _stream;
            _stream = mine;
            _outbox.RemoveAll(e => e.Seq <= after);
        }
        older?.Cancel();
        Interlocked.Increment(ref _streams);
        Seen();
        try
        {
            response.ContentType = "application/x-ndjson";
            response.Headers.CacheControl = "no-cache";
            response.Headers["X-Accel-Buffering"] = "no";
            response.HttpContext.Features.Get<Microsoft.AspNetCore.Http.Features.IHttpResponseBodyFeature>()?.DisableBuffering();
            await response.StartAsync(mine.Token);
            await response.Body.FlushAsync(mine.Token);
            var sent = after;
            var lines = new List<byte[]>();
            _bell.Writer.TryWrite(true); // whatever is already queued
            while (await _bell.Reader.WaitToReadAsync(mine.Token))
            {
                _bell.Reader.TryRead(out _);
                lines.Clear();
                lock (_outLock)
                {
                    foreach (var e in _outbox)
                        if (e.Seq > sent) lines.Add(e.Line);
                    if (_outbox.Count > 0) sent = Math.Max(sent, _outbox[^1].Seq);
                }
                foreach (var l in lines) await response.Body.WriteAsync(l, mine.Token);
                var wrote = lines.Count > 0;
                if (Interlocked.Exchange(ref _pingDue, 0) == 1 && !wrote)
                {
                    await response.Body.WriteAsync(PingLine, mine.Token);
                    wrote = true;
                }
                if (wrote)
                {
                    await response.Body.FlushAsync(mine.Token);
                    Interlocked.Exchange(ref _lastWriteTicks, DateTimeOffset.UtcNow.UtcTicks);
                }
            }
        }
        catch (OperationCanceledException) { }
        catch (IOException) { }
        finally
        {
            Seen();
            Interlocked.Decrement(ref _streams);
            lock (_outLock)
            {
                if (_stream == mine) _stream = null;
            }
            mine.Dispose();
        }
    }

    /// <summary>
    /// POST /agent/push: one segment of machine messages, one JSON per line. A seq this
    /// session already took is ignored (a retried push); a gap is logged.
    /// </summary>
    public async Task<bool> PushAsync(long seq, ReadOnlyMemory<byte> ndjson)
    {
        Seen();
        await _pushLock.WaitAsync();
        try
        {
            if (seq <= _lastPush) return false;
            if (seq != _lastPush + 1)
                _log.LogWarning("machine {Machine} push gap: got {Seq} after {Last}", Id, seq, _lastPush);
            _lastPush = seq;
            var rest = ndjson;
            while (!rest.IsEmpty)
            {
                var nl = rest.Span.IndexOf((byte)'\n');
                var line = nl < 0 ? rest : rest[..nl];
                rest = nl < 0 ? ReadOnlyMemory<byte>.Empty : rest[(nl + 1)..];
                if (line.Span.Trim((byte)'\r').IsEmpty) continue;
                try
                {
                    await HandleAsync(JsonNode.Parse(line.Span));
                }
                catch (Exception e) when (e is JsonException or InvalidOperationException or FormatException)
                {
                    _log.LogWarning(e, "bad message from {Machine}", Id);
                }
            }
            return true;
        }
        finally
        {
            _pushLock.Release();
        }
    }

    /// <summary>Ring the stream for a heartbeat line if it wrote nothing for <paramref name="every"/>.</summary>
    internal void PingIfQuiet(TimeSpan every)
    {
        if (Volatile.Read(ref _streams) == 0) return;
        if (DateTimeOffset.UtcNow.UtcTicks - Interlocked.Read(ref _lastWriteTicks) < every.Ticks) return;
        Interlocked.Exchange(ref _pingDue, 1);
        _bell.Writer.TryWrite(true);
    }

    internal void Close(string why)
    {
        _log.LogInformation("machine {Machine} session {Session} closed: {Why}", Id, SessionId, why);
        _closed.Cancel();
    }

    internal void Hello(JsonNode hello)
    {
        Protocol = hello["protocol"]?.GetValue<int>() ?? 0;
        Host = hello["machine_name"]?.GetValue<string>() ?? "";
        Version = hello["klark_version"]?.GetValue<string>() ?? "";
        _registry.Store.MachineConnected(Id, Host, Version);
        _registry.NotifyChanged();
    }

    /// <summary>The configured machine name the token belongs to; never what the machine claims.</summary>
    public string Id { get; }
    public string Name { get; }

    /// <summary>The host name the machine reported about itself (informational only).</summary>
    public string Host { get; private set; } = "";
    public string Version { get; private set; } = "";

    /// <summary>Link protocol from hello; 0 until hello arrives.</summary>
    public int Protocol { get; private set; }
    public DateTimeOffset ConnectedAt { get; } = DateTimeOffset.UtcNow;
    public SessionTree Tree { get; private set; } = SessionTree.Empty;

    /// <summary>When the tree was last pulled; null until someone first looked.</summary>
    public DateTimeOffset? TreeFetchedAt { get; private set; }

    private string? _treeRev;
    private TaskCompletionSource<bool>? _treePull;
    private readonly object _treeLock = new();

    /// <summary>
    /// Pull the pane tree if the copy here is older than <paramref name="maxAge"/>.
    /// Machines never push trees (ADR 0001, D11): this is the only way one
    /// arrives, and concurrent viewers share a single in-flight pull.
    /// </summary>
    public async Task EnsureTreeAsync(TimeSpan maxAge)
    {
        Task pull;
        lock (_treeLock)
        {
            if (TreeFetchedAt is { } at && DateTimeOffset.UtcNow - at < maxAge) return;
            if (_treePull is null)
            {
                _treePull = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
                _ = SendAsync(new { type = "get_tree", known_rev = _treeRev });
            }
            pull = _treePull.Task;
        }
        try
        {
            await pull.WaitAsync(TimeSpan.FromSeconds(10));
        }
        catch (TimeoutException)
        {
            // Let the next viewer ask again instead of waiting on a lost reply.
            lock (_treeLock)
            {
                if (_treePull?.Task == pull) _treePull = null;
            }
            throw;
        }
    }

    private void TreeArrived()
    {
        TaskCompletionSource<bool>? done;
        lock (_treeLock)
        {
            TreeFetchedAt = DateTimeOffset.UtcNow;
            done = _treePull;
            _treePull = null;
        }
        done?.TrySetResult(true);
    }

    /// <summary>
    /// Start watching a pane. The first viewer of a pane makes the machine start
    /// streaming it; disposing the last viewer stops the stream.
    /// </summary>
    /// <param name="onReset">
    /// Called with a fresh screen when this viewer fell too far behind and its
    /// queued output was dropped; the viewer should clear and repaint.
    /// </param>
    public IAsyncDisposable Watch(string paneId, Func<string, Task> onFrame, Func<string, Task> onClosed,
        Func<string, Task>? onReset = null)
    {
        var viewer = new PaneViewer(this, paneId, onFrame, onClosed, onReset, _log);
        bool first;
        lock (_subsLock)
        {
            if (!_viewers.TryGetValue(paneId, out var list))
            {
                list = new List<PaneViewer>();
                _viewers[paneId] = list;
            }
            first = list.Count == 0;
            list.Add(viewer);
        }
        if (first)
        {
            _ = SendAsync(new { type = "subscribe", pane_id = paneId });
        }
        return viewer;
    }

    private async Task UnwatchAsync(PaneViewer viewer)
    {
        bool last = false;
        lock (_subsLock)
        {
            if (_viewers.TryGetValue(viewer.PaneId, out var list) && list.Remove(viewer) && list.Count == 0)
            {
                _viewers.Remove(viewer.PaneId);
                last = true;
            }
        }
        if (last)
        {
            await SendAsync(new { type = "unsubscribe", pane_id = viewer.PaneId });
        }
    }

    public Task SendInputAsync(string paneId, string text) =>
        SendAsync(new { type = "input", pane_id = paneId, text });

    public Task SendKeysAsync(string paneId, params string[] keys) =>
        SendAsync(new { type = "keys", pane_id = paneId, keys });

    /// <summary>An allow-listed read on the machine (pane.read, agent.read, ...).</summary>
    public Task<JsonNode?> CallAsync(string method, object parameters, TimeSpan? timeout = null) =>
        RequestAsync(method, id => new { type = "call", id, method, @params = parameters }, timeout ?? TimeSpan.FromSeconds(15));

    /// <summary>The agents in every klark session on this machine (protocol 2, ADR 0009).</summary>
    public async Task<JsonArray> ListAgentsAsync(TimeSpan timeout)
    {
        RequireProtocol2();
        var result = await RequestAsync("list_agents", id => new { type = "list_agents", id }, timeout);
        return result as JsonArray ?? new JsonArray();
    }

    /// <summary>Hand a message to one agent on this machine (protocol 2, ADR 0009). Throws with the machine's error.</summary>
    public async Task DeliverAsync(string session, string agent, string from, string text, TimeSpan timeout)
    {
        RequireProtocol2();
        await RequestAsync("deliver", id => new { type = "deliver", id, session, agent, from, text }, timeout);
    }

    private void RequireProtocol2()
    {
        if (Protocol < 2)
            throw new NotSupportedException($"machine {Id} links with protocol {Protocol}; update klark there to receive messages");
    }

    private void RequireProtocol3(string what)
    {
        if (Protocol < 3)
            throw new NotSupportedException($"machine {Id} links with protocol {Protocol}; update klark there to {what}");
    }

    private readonly ConcurrentDictionary<string, (int Cols, int Rows)> _paneSizes = new();

    /// <summary>The last size the machine reported for a pane (protocol 3), or null.</summary>
    public (int Cols, int Rows)? PaneSize(string paneId) => _paneSizes.TryGetValue(paneId, out var s) ? s : null;

    /// <summary>Raised with (pane, cols, rows) when a subscribed pane reports a new size.</summary>
    public event Action<string, int, int>? PaneSizeChanged;

    /// <summary>Ask the machine to download an asset into a pane's agent (ADR 0011). Returns the path it wrote.</summary>
    public async Task<string> FetchAssetAsync(Asset asset, string caption, TimeSpan timeout)
    {
        RequireProtocol3("receive images");
        var result = await RequestAsync("fetch_asset", id => new
        {
            type = "fetch_asset", id, sha256 = asset.Sha256, name = asset.Name, mime = asset.Mime, size = asset.Size,
            pane_id = asset.PaneId, caption,
        }, timeout);
        return result?["path"]?.GetValue<string>() ?? "";
    }

    /// <summary>Structured transcript items newer than <paramref name="after"/> (protocol 3).</summary>
    public async Task<JsonNode?> GetTranscriptAsync(string paneId, string? after, TimeSpan timeout)
    {
        RequireProtocol3("read transcripts");
        return await RequestAsync("get_transcript", id => after is null
            ? new { type = "get_transcript", id, pane_id = paneId }
            : (object)new { type = "get_transcript", id, pane_id = paneId, after }, timeout);
    }

    /// <summary>Send a request with a fresh id and wait for the machine's `result {id, ...}`.</summary>
    private async Task<JsonNode?> RequestAsync(string what, Func<string, object> build, TimeSpan timeout)
    {
        var id = Interlocked.Increment(ref _nextCallId).ToString();
        var tcs = new TaskCompletionSource<JsonNode?>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[id] = tcs;
        try
        {
            await SendAsync(build(id));
            using var cts = new CancellationTokenSource(timeout);
            await using var reg = cts.Token.Register(() => tcs.TrySetException(new TimeoutException($"{what} timed out")));
            return await tcs.Task;
        }
        finally
        {
            _pending.TryRemove(id, out _);
        }
    }

    /// <summary>The pane's current screen with colours, so a new viewer is not blank until the next output.</summary>
    public async Task<string> ReadScreenAsync(string paneId)
    {
        var result = await CallAsync("pane.read", new { pane_id = paneId, source = "visible", format = "ansi", strip_ansi = false });
        return result?["read"]?["text"]?.GetValue<string>() ?? "";
    }

    private Task SendAsync(object message)
    {
        if (IsClosed) return Task.CompletedTask;
        var node = JsonSerializer.SerializeToNode(message) as JsonObject ?? new JsonObject();
        lock (_outLock)
        {
            var seq = ++_outSeq;
            node["seq"] = seq;
            var line = Encoding.UTF8.GetBytes(node.ToJsonString() + "\n");
            if (_outbox.Count >= OutboxLimit)
            {
                _log.LogWarning("machine {Machine} is not reading its event stream; dropping old messages", Id);
                _outbox.RemoveRange(0, _outbox.Count / 2);
            }
            _outbox.Add((seq, line));
        }
        _bell.Writer.TryWrite(true);
        return Task.CompletedTask;
    }

    internal void FailPending()
    {
        lock (_treeLock)
        {
            _treePull?.TrySetException(new InvalidOperationException("machine disconnected"));
            _treePull = null;
        }
        foreach (var tcs in _pending.Values)
        {
            tcs.TrySetException(new InvalidOperationException("machine disconnected"));
        }
        List<PaneViewer> viewers;
        lock (_subsLock)
        {
            viewers = _viewers.Values.SelectMany(v => v).ToList();
            _viewers.Clear();
        }
        foreach (var v in viewers)
        {
            v.Close("machine disconnected");
        }
    }

    /// <summary>klark-link PROTOCOL_VERSIONs this hub understands. v2 adds list_agents and deliver (ADR 0009).</summary>
    /// <remarks>v3 adds pane_size, fetch_asset and get_transcript (ADR 0011); v4 is v3 over plain HTTP (ADR 0013).</remarks>
    public const int MinProtocol = 4, MaxProtocol = 4;

    private async Task HandleAsync(JsonNode? msg)
    {
        switch (msg?["type"]?.GetValue<string>())
        {
            case "tree":
                Tree = SessionTree.Parse(msg["snapshot"]);
                _treeRev = msg["rev"]?.GetValue<string>();
                _registry.Store.SaveTree(Id, msg["snapshot"]?.ToJsonString() ?? "null");
                TreeArrived();
                _registry.NotifyChanged();
                break;
            case "pane_size":
                var sizePane = msg["pane_id"]?.GetValue<string>();
                var cols = msg["cols"]?.GetValue<int>() ?? 0;
                var rows = msg["rows"]?.GetValue<int>() ?? 0;
                if (sizePane is not null && cols is > 0 and <= 1000 && rows is > 0 and <= 1000)
                {
                    var changed = !_paneSizes.TryGetValue(sizePane, out var old) || old != (cols, rows);
                    _paneSizes[sizePane] = (cols, rows);
                    if (changed)
                    {
                        _log.LogInformation("pane_size {Machine}/{Pane} {Cols}x{Rows}", Id, sizePane, cols, rows);
                        try { PaneSizeChanged?.Invoke(sizePane, cols, rows); }
                        catch (Exception e) { _log.LogWarning(e, "pane_size handler failed"); }
                    }
                }
                break;
            case "tree_unchanged":
                TreeArrived();
                break;
            case "frame":
                // UTF-8 output arrives as "text" (ADR 0013); anything else as base64 "bytes".
                var framePane = msg["pane_id"]?.GetValue<string>();
                var raw = msg["text"]?.GetValue<string>() is { } text
                    ? Encoding.UTF8.GetBytes(text)
                    : Convert.FromBase64String(msg["bytes"]?.GetValue<string>() ?? "");
                if (framePane is null || raw.Length == 0) break;
                _registry.Store.AppendOutput(Id, framePane, raw);
                var b64 = Convert.ToBase64String(raw);
                Dispatch(framePane, v => v.Enqueue(b64));
                break;
            case "closed":
                var paneId = msg["pane_id"]?.GetValue<string>();
                var reason = msg["reason"]?.GetValue<string>() ?? "closed";
                Dispatch(paneId, v => v.Close(reason));
                lock (_subsLock)
                {
                    if (paneId is not null) _viewers.Remove(paneId);
                }
                break;
            case "result":
                var id = msg["id"]?.GetValue<string>();
                if (id is not null && _pending.TryGetValue(id, out var tcs))
                {
                    if (msg["error"]?.GetValue<string>() is { } error)
                        tcs.TrySetException(new InvalidOperationException(error));
                    else
                        tcs.TrySetResult(msg["result"]?.DeepClone());
                }
                break;
        }
    }

    /// <summary>
    /// Hand an event to every viewer of a pane without waiting for any of them:
    /// each viewer drains its own queue, so one slow browser never delays the
    /// machine's receive loop or the other viewers (STATUS S-13).
    /// </summary>
    private void Dispatch(string? paneId, Action<PaneViewer> action)
    {
        if (paneId is null) return;
        PaneViewer[] targets;
        lock (_subsLock)
        {
            targets = _viewers.TryGetValue(paneId, out var list) ? list.ToArray() : Array.Empty<PaneViewer>();
        }
        foreach (var v in targets) action(v);
    }

    private sealed class PaneViewer : IAsyncDisposable
    {
        /// <summary>~2.5 s of output at 50 ms batches; past that, drop and repaint.</summary>
        private const int QueueLimit = 50;

        private readonly MachineConnection _owner;
        private readonly Func<string, Task> _onFrame;
        private readonly Func<string, Task> _onClosed;
        private readonly Func<string, Task>? _onReset;
        private readonly ILogger _log;
        private readonly System.Threading.Channels.Channel<string> _queue =
            System.Threading.Channels.Channel.CreateBounded<string>(new System.Threading.Channels.BoundedChannelOptions(QueueLimit)
            {
                SingleReader = true,
                FullMode = System.Threading.Channels.BoundedChannelFullMode.Wait,
            });
        private int _resync;
        private int _disposed;
        private string? _closedReason;

        public PaneViewer(MachineConnection owner, string paneId, Func<string, Task> onFrame, Func<string, Task> onClosed,
            Func<string, Task>? onReset, ILogger log)
        {
            _owner = owner;
            PaneId = paneId;
            _onFrame = onFrame;
            _onClosed = onClosed;
            _onReset = onReset;
            _log = log;
            _ = PumpAsync();
        }

        public string PaneId { get; }

        public void Enqueue(string b64)
        {
            if (_queue.Writer.TryWrite(b64)) return;
            // Full: this browser can't keep up. Drop what is queued and repaint
            // it from the current screen instead of replaying stale output.
            Interlocked.Exchange(ref _resync, 1);
            while (_queue.Reader.TryRead(out _)) { }
            _queue.Writer.TryWrite("");
        }

        public void Close(string reason)
        {
            _closedReason = reason;
            _queue.Writer.TryComplete();
        }

        private async Task PumpAsync()
        {
            try
            {
                await foreach (var b64 in _queue.Reader.ReadAllAsync())
                {
                    if (Interlocked.Exchange(ref _resync, 0) == 1 && _onReset is not null)
                    {
                        var screen = await _owner.ReadScreenAsync(PaneId);
                        await _onReset(screen);
                        continue;
                    }
                    if (b64.Length > 0) await _onFrame(b64);
                }
                if (_closedReason is not null) await _onClosed(_closedReason);
            }
            catch (Exception e)
            {
                _log.LogDebug(e, "viewer of {Pane} stopped", PaneId);
            }
        }

        public ValueTask DisposeAsync()
        {
            if (Interlocked.Exchange(ref _disposed, 1) != 0) return ValueTask.CompletedTask;
            _queue.Writer.TryComplete();
            return new ValueTask(_owner.UnwatchAsync(this));
        }
    }
}
