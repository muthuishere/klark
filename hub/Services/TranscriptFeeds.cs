using System.Collections.Concurrent;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace KlarkHub.Services;

/// <summary>One transcript item, with its markdown rendered once on arrival and shared by every viewer.</summary>
public sealed record ChatItem(string Id, string Role, string Kind, string? Text, string? Tool, string? Input, string Summary,
    bool? Ok, DateTimeOffset? Ts, IReadOnlyList<MdSegment>? Md, string? ToolUseId = null);

/// <summary>
/// The chat view's data for one pane (ADR 0011, D). Kept in memory after the
/// last viewer leaves, so coming back paints at once and then asks only for
/// items after <see cref="Cursor"/>.
/// </summary>
public sealed class TranscriptFeed
{
    internal TranscriptFeed(string machineId, string paneId) => (MachineId, PaneId) = (machineId, paneId);

    public string MachineId { get; }
    public string PaneId { get; }
    public IReadOnlyList<ChatItem> Items { get; internal set; } = Array.Empty<ChatItem>();
    /// <summary>Bumped whenever anything a viewer draws changes; unchanged polls bump nothing.</summary>
    public long Version { get; internal set; }
    /// <summary>"jsonl" (structured log), "screen" (cleaned screen text), or "" before the first read.</summary>
    public string Source { get; internal set; } = "";
    public string Agent { get; internal set; } = "";
    public string Status { get; internal set; } = "";
    public string? Cursor { get; internal set; }
    public string? Error { get; internal set; }
    public TranscriptClean.BlockedPrompt? Blocked { get; internal set; }
    public DateTimeOffset LastUsed { get; internal set; } = DateTimeOffset.UtcNow;

    internal readonly List<Action> Viewers = new();
    internal bool Polling;
    internal bool NoStructured;
}

/// <summary>
/// One ~1 Hz poll per pane, shared by all its chat viewers and running only
/// while at least one is open (ADR 0002 lazy, ADR 0011 D).
/// </summary>
public sealed class TranscriptFeeds
{
    public const int MaxItems = 2000;
    private const int MaxToolText = 4000;
    private static readonly TimeSpan CallTimeout = TimeSpan.FromSeconds(8);

    private readonly ConcurrentDictionary<(string, string), TranscriptFeed> _feeds = new();
    private readonly MachineRegistry _registry;
    private readonly ILogger<TranscriptFeeds> _log;

    public TranscriptFeeds(MachineRegistry registry, ILogger<TranscriptFeeds> log)
    {
        _registry = registry;
        _log = log;
    }

    public TranscriptFeed Get(string machineId, string paneId) =>
        _feeds.GetOrAdd((machineId, paneId), k => new TranscriptFeed(k.Item1, k.Item2));

    /// <summary>Start (or join) the pane's poll. Dispose to leave; the last one out stops it.</summary>
    public IDisposable Watch(string machineId, string paneId, Action onChange)
    {
        var feed = Get(machineId, paneId);
        bool start;
        lock (feed.Viewers)
        {
            feed.Viewers.Add(onChange);
            start = !feed.Polling;
            feed.Polling = true;
        }
        if (start)
        {
            _log.LogInformation("transcript poll of {Machine}/{Pane} started", machineId, paneId);
            _ = Task.Run(() => PollLoopAsync(feed));
        }
        return new Lease(() =>
        {
            lock (feed.Viewers) feed.Viewers.Remove(onChange);
        });
        // Old feeds are dropped when they have not been looked at for a day.
    }

    private sealed class Lease(Action dispose) : IDisposable
    {
        private int _done;
        public void Dispose()
        {
            if (Interlocked.Exchange(ref _done, 1) == 0) dispose();
        }
    }

    private async Task PollLoopAsync(TranscriptFeed feed)
    {
        using var timer = new PeriodicTimer(TimeSpan.FromSeconds(1));
        try
        {
            do
            {
                lock (feed.Viewers)
                {
                    if (feed.Viewers.Count == 0)
                    {
                        feed.Polling = false;
                        break;
                    }
                }
                feed.LastUsed = DateTimeOffset.UtcNow;
                var before = feed.Version;
                try { await PollOnceAsync(feed); }
                catch (Exception e) { _log.LogWarning(e, "transcript poll of {Pane} failed", feed.PaneId); }
                if (feed.Version != before) Notify(feed);
            } while (await timer.WaitForNextTickAsync());
        }
        finally
        {
            lock (feed.Viewers) feed.Polling = false;
            Evict();
        }
    }

    private static void Notify(TranscriptFeed feed)
    {
        Action[] viewers;
        lock (feed.Viewers) viewers = feed.Viewers.ToArray();
        foreach (var v in viewers)
        {
            try { v(); } catch { /* a closing viewer */ }
        }
    }

    private void Evict()
    {
        var old = DateTimeOffset.UtcNow.AddHours(-24);
        foreach (var (key, f) in _feeds)
        {
            lock (f.Viewers)
            {
                if (f.Viewers.Count == 0 && !f.Polling && f.LastUsed < old) _feeds.TryRemove(key, out _);
            }
        }
    }

    internal async Task PollOnceAsync(TranscriptFeed feed)
    {
        var machine = _registry.Get(feed.MachineId);
        if (machine is null)
        {
            Set(feed, error: "machine offline");
            return;
        }
        try { await machine.EnsureTreeAsync(TimeSpan.FromSeconds(2)); } catch (Exception e) when (e is TimeoutException or InvalidOperationException) { }
        var pane = machine.Tree.Panes.FirstOrDefault(p => p.Id == feed.PaneId);
        if (pane is not null && pane.AgentStatus != feed.Status)
        {
            feed.Status = pane.AgentStatus;
            feed.Version++;
        }

        var structured = false;
        if (machine.Protocol >= 3 && !feed.NoStructured)
        {
            try
            {
                var result = await machine.GetTranscriptAsync(feed.PaneId, feed.Cursor, CallTimeout);
                Apply(feed, result);
                structured = true;
            }
            catch (InvalidOperationException e) when (e.Message.Contains("unknown", StringComparison.OrdinalIgnoreCase)
                                                       || e.Message.Contains("unsupported", StringComparison.OrdinalIgnoreCase))
            {
                feed.NoStructured = true;
            }
            catch (Exception e) when (e is TimeoutException or InvalidOperationException)
            {
                Set(feed, error: e.Message);
            }
        }
        if (!structured && (machine.Protocol < 3 || feed.NoStructured))
        {
            await ReadScreenTextAsync(machine, feed);
        }

        // A blocked agent's question comes from its screen, where the options are drawn.
        TranscriptClean.BlockedPrompt? blocked = null;
        if (feed.Status == "blocked")
        {
            try
            {
                var r = await machine.CallAsync("pane.read", new { pane_id = feed.PaneId, source = "visible", format = "text", strip_ansi = true }, CallTimeout);
                var text = TranscriptClean.Clean(r?["read"]?["text"]?.GetValue<string>() ?? "");
                if (text.Length > 0) blocked = TranscriptClean.Blocked(text);
            }
            catch (Exception e) when (e is TimeoutException or InvalidOperationException) { }
        }
        if (!SameBlocked(blocked, feed.Blocked))
        {
            feed.Blocked = blocked;
            feed.Version++;
        }
    }

    private static bool SameBlocked(TranscriptClean.BlockedPrompt? a, TranscriptClean.BlockedPrompt? b) =>
        a is null ? b is null : b is not null && a.Question == b.Question && a.Options.SequenceEqual(b.Options);

    private static void Set(TranscriptFeed feed, string? error)
    {
        if (feed.Error == error) return;
        feed.Error = error;
        feed.Version++;
    }

    /// <summary>Protocol &lt; 3, or no get_transcript: agent.read, else pane.read recent_unwrapped, cleaned.</summary>
    private static async Task ReadScreenTextAsync(MachineConnection machine, TranscriptFeed feed)
    {
        string? text;
        try
        {
            var r = await machine.CallAsync("agent.read", new { target = feed.PaneId, source = "recent", format = "text", strip_ansi = true, lines = 400 }, CallTimeout);
            text = r?["read"]?["text"]?.GetValue<string>();
        }
        catch (Exception e) when (e is TimeoutException or InvalidOperationException)
        {
            text = null;
        }
        if (string.IsNullOrEmpty(text))
        {
            try
            {
                var r = await machine.CallAsync("pane.read", new { pane_id = feed.PaneId, source = "recent_unwrapped", format = "text", strip_ansi = true, lines = 400 }, CallTimeout);
                text = r?["read"]?["text"]?.GetValue<string>() ?? "";
            }
            catch (Exception e) when (e is TimeoutException or InvalidOperationException)
            {
                Set(feed, error: e.Message);
                return;
            }
        }
        SetScreen(feed, text);
    }

    private static void SetScreen(TranscriptFeed feed, string raw)
    {
        var cleaned = TranscriptClean.Clean(raw);
        Set(feed, error: null);
        if (feed.Source == "screen" && feed.Items.Count == 1 && feed.Items[0].Text == cleaned) return;
        feed.Source = "screen";
        feed.Cursor = null;
        feed.Items = new[]
        {
            new ChatItem("screen", "assistant", "text", cleaned, null, null, "", null, null,
                cleaned.Length == 0 ? Array.Empty<MdSegment>() : ChatMarkdown.Render(cleaned, fromScreen: true)),
        };
        feed.Version++;
    }

    /// <summary>Merge a get_transcript result: append new items after the cursor; a screen result replaces all.</summary>
    internal static void Apply(TranscriptFeed feed, JsonNode? result)
    {
        Set(feed, error: null);
        var source = Str(result?["source"]) ?? "jsonl";
        var agent = Str(result?["agent"]) ?? "";
        if (agent != feed.Agent)
        {
            feed.Agent = agent;
            feed.Version++;
        }
        var items = (result?["items"] as JsonArray)?.OfType<JsonObject>().ToList() ?? new List<JsonObject>();
        if (source == "screen")
        {
            var text = string.Join("\n\n", items.Select(i => Str(i["text"]) ?? "").Where(t => t.Length > 0));
            SetScreen(feed, text);
            feed.Cursor = Str(result?["cursor"]);
            return;
        }
        var cursor = Str(result?["cursor"]);
        var replace = feed.Source != "jsonl";
        if (!replace && items.Count == 0 && cursor == feed.Cursor) return;

        var list = replace ? new List<ChatItem>() : feed.Items.ToList();
        var seen = list.Select(i => i.Id).ToHashSet();
        var n = list.Count;
        foreach (var o in items)
        {
            var item = ToItem(o, n++);
            if (item is null || !seen.Add(item.Id)) continue;
            list.Add(item);
        }
        if (list.Count > MaxItems) list = list.GetRange(list.Count - MaxItems, MaxItems);
        feed.Source = "jsonl";
        feed.Cursor = cursor;
        if (replace || list.Count != feed.Items.Count)
        {
            feed.Items = list;
            feed.Version++;
        }
    }

    private static ChatItem? ToItem(JsonObject o, int index)
    {
        var role = Str(o["role"]) ?? "assistant";
        var kind = Str(o["kind"]) ?? "text";
        var id = Str(o["id"]) ?? $"i{index}";
        var text = Str(o["text"]);
        var tool = Str(o["tool"]);
        var input = o["input"];
        bool? ok = o["ok"] is JsonValue v && v.TryGetValue<bool>(out var b) ? b : null;
        DateTimeOffset? ts = null;
        if (o["ts"] is JsonValue tv)
        {
            if (tv.TryGetValue<string>(out var s) && DateTimeOffset.TryParse(s, out var p)) ts = p;
            else if (tv.TryGetValue<long>(out var ms)) ts = ms > 10_000_000_000 ? DateTimeOffset.FromUnixTimeMilliseconds(ms) : DateTimeOffset.FromUnixTimeSeconds(ms);
        }
        string? inputText = input switch
        {
            null => null,
            JsonValue jv when jv.TryGetValue<string>(out var s) => s,
            _ => input.ToJsonString(new JsonSerializerOptions { WriteIndented = true }),
        };
        if (kind is "tool_use" or "tool_result")
        {
            text = Cap(text);
            inputText = Cap(inputText);
        }
        IReadOnlyList<MdSegment>? md = kind == "text" && role == "assistant" && !string.IsNullOrEmpty(text) ? ChatMarkdown.Render(text) : null;
        return new ChatItem(id, role, kind, text, tool, inputText, Summarize(tool, input), ok, ts, md, Str(o["tool_use_id"]));
    }

    private static string? Cap(string? s) =>
        s is { Length: > MaxToolText } ? s[..MaxToolText] + $"\n… ({s.Length - MaxToolText} more characters)" : s;

    private static readonly string[] SummaryKeys = ["command", "file_path", "path", "pattern", "url", "query", "description", "prompt", "notebook_path"];

    /// <summary>"cargo test", "src/x.rs": the one argument that says what a tool call did.</summary>
    internal static string Summarize(string? tool, JsonNode? input)
    {
        string? arg = null;
        if (input is JsonObject obj)
        {
            foreach (var k in SummaryKeys)
            {
                if (Str(obj[k]) is { Length: > 0 } s)
                {
                    arg = s;
                    break;
                }
            }
        }
        else if (input is JsonValue jv && jv.TryGetValue<string>(out var s))
        {
            arg = s;
        }
        if (arg is null) return "";
        arg = arg.ReplaceLineEndings(" ").Trim();
        return arg.Length > 70 ? arg[..67] + "…" : arg;
    }

    private static string? Str(JsonNode? n) => n is JsonValue v && v.TryGetValue<string>(out var s) ? s : null;
}
