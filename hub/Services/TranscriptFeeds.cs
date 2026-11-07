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
    internal string? ScreenText { get; set; }
    public string? Error { get; internal set; }
    public TranscriptClean.BlockedPrompt? Blocked { get; internal set; }
    /// <summary>The pane's agent as the machine's tree names it ("" for a plain shell).</summary>
    public string PaneAgent { get; internal set; } = "";
    /// <summary>"jsonl"/"db" when structured, else the screen transformer that read it: "claude", "devin", "generic".</summary>
    public string Reader { get; internal set; } = "";
    /// <summary>When the items last changed, for the sidebar's "2m ago".</summary>
    public DateTimeOffset? LastActivity { get; internal set; }
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

    /// <summary>Feeds held in memory (every chat opened since the hub started, for a day).</summary>
    public IEnumerable<TranscriptFeed> Loaded => _feeds.Values;

    public sealed record Hit(string MachineId, string PaneId, string ItemId, string Role, string Before, string Match, string After);

    /// <summary>
    /// Message text search over loaded chats, for the sidebar: every word must appear in one
    /// item; returns a snippet around the first word. Newest items first, at most
    /// <paramref name="limit"/> hits and 3 per chat.
    /// </summary>
    public List<Hit> Search(string query, Func<string, bool> canSee, int limit = 30)
    {
        var words = query.Split(' ', StringSplitOptions.RemoveEmptyEntries);
        var hits = new List<Hit>();
        if (words.Length == 0 || query.Trim().Length < 2) return hits;
        foreach (var f in _feeds.Values.OrderByDescending(f => f.LastActivity))
        {
            if (!canSee(f.MachineId)) continue;
            var per = 0;
            var items = f.Items;
            for (var i = items.Count - 1; i >= 0 && per < 3 && hits.Count < limit; i--)
            {
                var it = items[i];
                if (it.Kind is not ("text" or "command" or "output") || string.IsNullOrEmpty(it.Text)) continue;
                if (!words.All(w => it.Text.Contains(w, StringComparison.OrdinalIgnoreCase))) continue;
                var at = it.Text.IndexOf(words[0], StringComparison.OrdinalIgnoreCase);
                var start = Math.Max(0, at - 40);
                var end = Math.Min(it.Text.Length, at + words[0].Length + 60);
                string Flat(string s) => s.ReplaceLineEndings(" ");
                hits.Add(new Hit(f.MachineId, f.PaneId, it.Id, it.Role, (start > 0 ? "…" : "") + Flat(it.Text[start..at]),
                    Flat(it.Text.Substring(at, words[0].Length)), Flat(it.Text[(at + words[0].Length)..end]) + (end < it.Text.Length ? "…" : "")));
                per++;
            }
            if (hits.Count >= limit) break;
        }
        return hits;
    }

    /// <summary>The last thing said in a chat, one line, for the sidebar preview.</summary>
    public static string Preview(TranscriptFeed? f)
    {
        if (f is null) return "";
        for (var i = f.Items.Count - 1; i >= 0; i--)
        {
            var it = f.Items[i];
            if (it.Kind is "text" or "command" && !string.IsNullOrWhiteSpace(it.Text))
            {
                var line = it.Text.Split('\n').Select(l => l.Trim().TrimStart('#', '>', '-', '*', ' ')).FirstOrDefault(l => l.Length > 0 && !l.StartsWith("|")) ?? "";
                return (it.Role == "user" ? "You: " : "") + (line.Length > 120 ? line[..117] + "…" : line);
            }
        }
        return "";
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
        if (pane is not null) feed.PaneAgent = pane.Agent;
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

    internal static void SetScreen(TranscriptFeed feed, string raw)
    {
        Set(feed, error: null);
        if (feed.Source == "screen" && feed.ScreenText == raw) return;
        var agent = feed.Agent.Length > 0 ? feed.Agent : feed.PaneAgent;
        var kind = ScreenTransformers.Pick(agent, raw);
        feed.Source = "screen";
        feed.Reader = kind.ToString().ToLowerInvariant();
        feed.ScreenText = raw;
        feed.Cursor = null;
        var items = ScreenItems(ScreenTransformers.Transform(agent, raw), feed.MachineId, feed.PaneId);
        if (!items.Select(i => i.Id).SequenceEqual(feed.Items.Select(i => i.Id)) || items.Count == 0) feed.LastActivity = DateTimeOffset.UtcNow;
        feed.Items = items;
        feed.Version++;
    }

    /// <summary>
    /// A cleaned screen as turns: the person's prompts, the agent's text, tool calls with
    /// their results. Ids come from each turn's first line and its occurrence, so a row
    /// keeps its identity (and its opened state) while the screen grows under it.
    /// </summary>
    internal static List<ChatItem> ScreenItems(string cleaned) => ScreenItems(TranscriptClean.Turns(cleaned));

    internal static List<ChatItem> ScreenItems(List<TranscriptClean.ScreenTurn> turns, string? machine = null, string? pane = null)
    {
        var items = new List<ChatItem>();
        var seen = new Dictionary<string, int>();
        foreach (var t in turns)
        {
            var plain = t.Kind == "output" ? TranscriptClean.StripAnsi(t.Text) : t.Text;
            var head = t.Kind == "tool_use" ? $"{t.Tool}({t.Summary})" : plain.Split('\n')[0];
            var baseId = $"s:{t.Kind}:{(uint)StableHash(head):x8}";
            seen[baseId] = seen.TryGetValue(baseId, out var n) ? n + 1 : 0;
            var id = $"{baseId}:{seen[baseId]}";
            IReadOnlyList<MdSegment>? md = t.Kind switch
            {
                "text" when t.Role == "assistant" => Link(ChatMarkdown.Render(t.Text, fromScreen: true), machine, pane),
                "output" => [new MdAnsi(Linkify(AnsiHtml.Render(t.Text), machine, pane), plain.Split('\n').Length)],
                _ => null,
            };
            // A screen shows no exit status; an error-shaped result is the best evidence there is.
            bool? ok = t.Kind == "tool_result" ? !ErrorShaped(plain) : null;
            items.Add(new ChatItem(id, t.Role, t.Kind, t.Kind == "tool_use" ? null : plain, t.Tool, null, t.Summary, ok, null, md));
        }
        return items;
    }

    private static bool ErrorShaped(string s) =>
        s.StartsWith("Error", StringComparison.OrdinalIgnoreCase) || s.Contains("FAILED") || s.Contains("error[") || s.Contains("panicked at");

    private static string Linkify(string html, string? machine, string? pane) =>
        machine is null || pane is null ? html : FileLinks.Linkify(html, machine, pane);

    /// <summary>File paths in rendered markdown become viewer links (ADR 0011, G).</summary>
    internal static IReadOnlyList<MdSegment> Link(IReadOnlyList<MdSegment> md, string? machine, string? pane) =>
        machine is null || pane is null ? md : md.Select(s => s is MdHtml h ? new MdHtml(FileLinks.Linkify(h.Html, machine, pane)) : s).ToList();

    private static int StableHash(string s)
    {
        unchecked
        {
            var h = (int)2166136261;
            foreach (var c in s) h = (h ^ c) * 16777619;
            return h;
        }
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
            if (item?.Md is { } md) item = item with { Md = Link(md, feed.MachineId, feed.PaneId) };
            if (item is null || !seen.Add(item.Id)) continue;
            list.Add(item);
        }
        if (list.Count > MaxItems) list = list.GetRange(list.Count - MaxItems, MaxItems);
        feed.Source = "jsonl";
        feed.Reader = source;
        feed.Cursor = cursor;
        if (replace || list.Count != feed.Items.Count)
        {
            feed.LastActivity = list.LastOrDefault()?.Ts ?? DateTimeOffset.UtcNow;
            feed.Items = list;
            feed.Version++;
        }
    }

    internal static ChatItem? ToItem(JsonObject o, int index)
    {
        var role = Str(o["role"]) ?? "assistant";
        var kind = Str(o["kind"]) ?? "text";
        // Only what a person said, what the agent said, and its tool work is shown:
        // system/meta items, harness wrappers and empty items never reach the chat.
        if (role is not ("user" or "assistant" or "tool")) return null;
        if (role == "tool" && kind != "tool_result") return null;
        if (kind is not ("text" or "tool_use" or "tool_result" or "thinking")) return null;
        if (Flag(o["meta"]) || Flag(o["is_meta"]) || Flag(o["isMeta"])) return null;
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
        if (kind is "text" or "thinking")
        {
            text = TranscriptClean.CleanStructured(text);
            if (text.Length == 0) return null;
        }
        else if (kind == "tool_result" && text is not null)
        {
            text = TranscriptClean.CleanStructured(text);
        }
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

    private static bool Flag(JsonNode? n) => n is JsonValue v && v.TryGetValue<bool>(out var b) && b;

    private static string? Str(JsonNode? n) => n is JsonValue v && v.TryGetValue<string>(out var s) ? s : null;
}
