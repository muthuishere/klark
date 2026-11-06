using System.Collections.Concurrent;
using System.IO.Compression;
using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace KlarkHub.Services;

/// <summary>
/// Every klark machine currently linked to this hub. One instance for the whole
/// server, shared by all signed-in users: that is what lets many people gather
/// on one hub and see every machine's panes.
/// </summary>
public sealed class MachineRegistry
{
    private readonly ConcurrentDictionary<string, MachineConnection> _machines = new();
    private readonly ILogger<MachineRegistry> _log;
    private readonly HubStore _store;

    public MachineRegistry(ILogger<MachineRegistry> log, HubStore store)
    {
        _log = log;
        _store = store;
    }

    internal HubStore Store => _store;

    /// <summary>Raised when a machine connects, disconnects, or sends a new tree.</summary>
    public event Action? Changed;

    public IReadOnlyCollection<MachineConnection> Machines =>
        _machines.Values.OrderBy(m => m.Name, StringComparer.OrdinalIgnoreCase).ToList();

    public MachineConnection? Get(string id) => _machines.TryGetValue(id, out var m) ? m : null;

    /// <summary>Pull every connected machine's tree that is older than maxAge, in parallel.</summary>
    public Task RefreshTreesAsync(TimeSpan maxAge) =>
        Task.WhenAll(_machines.Values.Select(async m =>
        {
            try { await m.EnsureTreeAsync(maxAge); }
            catch (Exception e) when (e is TimeoutException or InvalidOperationException) { }
        }));

    internal void NotifyChanged()
    {
        try { Changed?.Invoke(); }
        catch (Exception e) { _log.LogWarning(e, "change handler failed"); }
    }

    /// <summary>Serve one machine's WebSocket until it closes.</summary>
    public async Task RunAsync(WebSocket socket, string machineId, CancellationToken ct)
    {
        var conn = new MachineConnection(machineId, socket, this, _log);
        if (_machines.TryGetValue(conn.Id, out var previous))
        {
            _log.LogInformation("machine {Machine} reconnected; dropping the old link", conn.Id);
            await previous.CloseAsync();
        }
        _machines[conn.Id] = conn;
        NotifyChanged();
        try
        {
            await conn.ReceiveLoopAsync(ct);
        }
        finally
        {
            _machines.TryRemove(new KeyValuePair<string, MachineConnection>(conn.Id, conn));
            conn.FailPending();
            _store.MachineDisconnected(conn.Id);
            NotifyChanged();
        }
    }
}

/// <summary>One linked klark machine.</summary>
public sealed class MachineConnection
{
    private readonly WebSocket _socket;
    private readonly MachineRegistry _registry;
    private readonly ILogger _log;
    private readonly SemaphoreSlim _sendLock = new(1, 1);
    private readonly object _subsLock = new();
    private readonly Dictionary<string, List<PaneViewer>> _viewers = new();
    private readonly ConcurrentDictionary<string, TaskCompletionSource<JsonNode?>> _pending = new();
    private long _nextCallId;

    internal MachineConnection(string id, WebSocket socket, MachineRegistry registry, ILogger log)
    {
        Id = id;
        Name = id;
        _socket = socket;
        _registry = registry;
        _log = log;
    }

    /// <summary>The configured machine name the token belongs to; never what the machine claims.</summary>
    public string Id { get; }
    public string Name { get; }

    /// <summary>The host name the machine reported about itself (informational only).</summary>
    public string Host { get; private set; } = "";
    public string Version { get; private set; } = "";
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
    public IAsyncDisposable Watch(string paneId, Func<string, Task> onFrame, Func<string, Task> onClosed)
    {
        var viewer = new PaneViewer(this, paneId, onFrame, onClosed);
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
    public async Task<JsonNode?> CallAsync(string method, object parameters, TimeSpan? timeout = null)
    {
        var id = Interlocked.Increment(ref _nextCallId).ToString();
        var tcs = new TaskCompletionSource<JsonNode?>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[id] = tcs;
        try
        {
            await SendAsync(new { type = "call", id, method, @params = parameters });
            using var cts = new CancellationTokenSource(timeout ?? TimeSpan.FromSeconds(15));
            await using var reg = cts.Token.Register(() => tcs.TrySetException(new TimeoutException($"{method} timed out")));
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

    private async Task SendAsync(object message)
    {
        var bytes = JsonSerializer.SerializeToUtf8Bytes(message);
        await _sendLock.WaitAsync();
        try
        {
            if (_socket.State == WebSocketState.Open)
            {
                await _socket.SendAsync(bytes, WebSocketMessageType.Text, true, CancellationToken.None);
            }
        }
        catch (Exception e)
        {
            _log.LogDebug(e, "send to {Machine} failed", Id);
        }
        finally
        {
            _sendLock.Release();
        }
    }

    internal async Task CloseAsync()
    {
        try
        {
            await _socket.CloseAsync(WebSocketCloseStatus.NormalClosure, "replaced", CancellationToken.None);
        }
        catch (Exception)
        {
            _socket.Abort();
        }
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
            _ = v.OnClosed("machine disconnected");
        }
    }

    internal async Task ReceiveLoopAsync(CancellationToken ct)
    {
        var buffer = new byte[64 * 1024];
        using var message = new MemoryStream();
        while (!ct.IsCancellationRequested && _socket.State == WebSocketState.Open)
        {
            WebSocketReceiveResult result;
            message.SetLength(0);
            do
            {
                result = await _socket.ReceiveAsync(buffer, ct);
                if (result.MessageType == WebSocketMessageType.Close)
                {
                    return;
                }
                message.Write(buffer, 0, result.Count);
                if (message.Length > 64 * 1024 * 1024)
                {
                    throw new InvalidOperationException("message too large");
                }
            } while (!result.EndOfMessage);

            try
            {
                if (result.MessageType == WebSocketMessageType.Binary && message.Length >= 2 &&
                    message.GetBuffer()[0] is FrameGzip or FrameRaw)
                {
                    await HandleBinaryFrameAsync(message);
                    continue;
                }
                await HandleAsync(JsonNode.Parse(Decode(message, result.MessageType)));
            }
            catch (Exception e) when (e is JsonException or InvalidOperationException or FormatException or InvalidDataException)
            {
                _log.LogWarning(e, "bad message from {Machine}", Id);
            }
        }
    }

    private const byte FrameGzip = 0x01;
    private const byte FrameRaw = 0x02;

    /// <summary>
    /// Terminal output: [kind][id length][pane id][raw or gzip bytes] (ADR 0001, D5).
    /// No base64 and no JSON on the hot path.
    /// </summary>
    private async Task HandleBinaryFrameAsync(MemoryStream message)
    {
        var buf = message.GetBuffer();
        var len = (int)message.Length;
        int idLen = buf[1];
        if (2 + idLen > len) throw new InvalidDataException("short frame");
        var paneId = Encoding.UTF8.GetString(buf, 2, idLen);
        byte[] raw;
        if (buf[0] == FrameGzip)
        {
            using var src = new MemoryStream(buf, 2 + idLen, len - 2 - idLen, writable: false);
            using var gz = new GZipStream(src, CompressionMode.Decompress);
            using var dst = new MemoryStream();
            gz.CopyTo(dst);
            raw = dst.ToArray();
        }
        else
        {
            raw = buf.AsSpan(2 + idLen, len - 2 - idLen).ToArray();
        }
        _registry.Store.AppendOutput(Id, paneId, raw);
        var b64 = Convert.ToBase64String(raw);
        await DispatchAsync(paneId, v => v.OnFrame(b64));
    }

    /// <summary>Machines send gzip JSON in binary frames (ADR 0001, D5); text frames are plain JSON.</summary>
    private static byte[] Decode(MemoryStream message, WebSocketMessageType type)
    {
        message.Position = 0;
        if (type != WebSocketMessageType.Binary) return message.ToArray();
        using var gz = new GZipStream(message, CompressionMode.Decompress, leaveOpen: true);
        using var json = new MemoryStream();
        gz.CopyTo(json);
        if (json.Length > 64 * 1024 * 1024) throw new InvalidDataException("message too large");
        return json.ToArray();
    }

    private async Task HandleAsync(JsonNode? msg)
    {
        switch (msg?["type"]?.GetValue<string>())
        {
            case "hello":
                Host = msg["machine_name"]?.GetValue<string>() ?? "";
                Version = msg["klark_version"]?.GetValue<string>() ?? "";
                _registry.Store.MachineConnected(Id, Host, Version);
                _registry.NotifyChanged();
                break;
            case "tree":
                Tree = SessionTree.Parse(msg["snapshot"]);
                _treeRev = msg["rev"]?.GetValue<string>();
                _registry.Store.SaveTree(Id, msg["snapshot"]?.ToJsonString() ?? "null");
                TreeArrived();
                _registry.NotifyChanged();
                break;
            case "tree_unchanged":
                TreeArrived();
                break;
            case "frame":
                var framePane = msg["pane_id"]?.GetValue<string>();
                var b64 = msg["bytes"]?.GetValue<string>() ?? "";
                if (framePane is not null && b64.Length > 0)
                {
                    _registry.Store.AppendOutput(Id, framePane, Convert.FromBase64String(b64));
                }
                await DispatchAsync(framePane, v => v.OnFrame(b64));
                break;
            case "closed":
                var paneId = msg["pane_id"]?.GetValue<string>();
                var reason = msg["reason"]?.GetValue<string>() ?? "closed";
                await DispatchAsync(paneId, v => v.OnClosed(reason));
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

    private async Task DispatchAsync(string? paneId, Func<PaneViewer, Task> action)
    {
        if (paneId is null) return;
        PaneViewer[] targets;
        lock (_subsLock)
        {
            targets = _viewers.TryGetValue(paneId, out var list) ? list.ToArray() : Array.Empty<PaneViewer>();
        }
        foreach (var v in targets)
        {
            try { await action(v); }
            catch (Exception e) { _log.LogDebug(e, "viewer of {Pane} failed", paneId); }
        }
    }

    private sealed class PaneViewer : IAsyncDisposable
    {
        private readonly MachineConnection _owner;
        private int _disposed;

        public PaneViewer(MachineConnection owner, string paneId, Func<string, Task> onFrame, Func<string, Task> onClosed)
        {
            _owner = owner;
            PaneId = paneId;
            OnFrame = onFrame;
            OnClosed = onClosed;
        }

        public string PaneId { get; }
        public Func<string, Task> OnFrame { get; }
        public Func<string, Task> OnClosed { get; }

        public ValueTask DisposeAsync() =>
            Interlocked.Exchange(ref _disposed, 1) == 0 ? new ValueTask(_owner.UnwatchAsync(this)) : ValueTask.CompletedTask;
    }
}
