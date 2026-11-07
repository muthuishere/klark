using System.Text;
using System.Text.RegularExpressions;
using static KlarkHub.Services.TranscriptClean;

namespace KlarkHub.Services;

/// <summary>Which screen transformer read a pane (ADR 0011, F).</summary>
public enum ScreenKind { Claude, Devin, Generic }

/// <summary>
/// Turn an agent's SCREEN into chat turns when no structured log exists (ADR 0011, F).
/// Three transformers: <b>Claude</b> (Claude Code: ⏺ turns, ⎿ results, ❯/&gt; prompts),
/// <b>Devin</b> (Devin/Codex: • turns, └ results, › prompts) and <b>Generic</b> (any other
/// agent or a plain shell: chrome stripped, the pane's own hard wrap undone, shell prompts
/// split into command turns, command output kept with its ANSI colours, prose shaped into
/// paragraphs, lists, rules and code blocks). Box-drawn tables become markdown tables in
/// all three.
///
/// Behaviour ported from herdr-expose (MIT, same author): internal/core/clean.go (chrome),
/// web/src/components/transcriptText.ts (shapeTranscript: wrap rejoin, code runs, bullets,
/// rules) and web/src/ansi/ansiToHtml.ts (SGR colours). Re-implemented in C#, not copied.
/// </summary>
public static partial class ScreenTransformers
{
    public static ScreenKind Pick(string? agent, string raw)
    {
        var a = (agent ?? "").ToLowerInvariant();
        if (a.Contains("claude")) return ScreenKind.Claude;
        if (a.Contains("devin") || a.Contains("codex")) return ScreenKind.Devin;
        // No agent named: judge by the screen's own markers.
        var plain = StripAnsi(raw);
        if (plain.Contains('⏺') || plain.Contains('⎿')) return ScreenKind.Claude;
        if (DevinAgentLine().IsMatch(plain) && DevinResultLine().IsMatch(plain)) return ScreenKind.Devin;
        return ScreenKind.Generic;
    }

    [GeneratedRegex(@"^• \S", RegexOptions.Multiline)] private static partial Regex DevinAgentLine();
    [GeneratedRegex(@"^\s{1,6}└ ", RegexOptions.Multiline)] private static partial Regex DevinResultLine();

    public static List<ScreenTurn> Transform(string? agent, string raw) => Pick(agent, raw) switch
    {
        ScreenKind.Claude => Claude(raw),
        ScreenKind.Devin => Devin(raw),
        _ => Generic(raw),
    };

    public static List<ScreenTurn> Claude(string raw) => Turns(BoxTables(Clean(raw)), ">❯", "⏺●", "⎿");

    public static List<ScreenTurn> Devin(string raw) => Turns(BoxTables(Clean(raw)), "›>", "•", "└");

    // ---- box-drawn tables -------------------------------------------------------------

    private static bool IsTableTop(string t) => t.Length > 2 && t[0] == '┌' && t[^1] == '┐' && t.Contains('┬');
    private static bool IsTableMid(string t) => t.Length > 2 && t[0] == '├' && t[^1] == '┤' && t.Contains('┼');
    private static bool IsTableBottom(string t) => t.Length > 2 && t[0] == '└' && t[^1] == '┘' && t.Contains('┴');
    private static bool IsTableRow(string t) => t.Length > 2 && t[0] == '│' && t[^1] == '│' && t.Count(c => c == '│') >= 3;

    /// <summary>
    /// A TUI draws a markdown table with box characters (┌─┬─┐ │ a │ b │ ├─┼─┤ └─┴─┘), and a
    /// cell that does not fit wraps onto more lines between the same separators. Each such
    /// table becomes a pipe table, keeping its indent, so markdown renders a real table.
    /// </summary>
    public static string BoxTables(string text)
    {
        var lines = text.Split('\n');
        var output = new List<string>(lines.Length);
        var fenced = false;
        for (var i = 0; i < lines.Length; i++)
        {
            var t = lines[i].Trim();
            if (t.StartsWith("```")) fenced = !fenced;
            if (fenced || !IsTableTop(t))
            {
                output.Add(lines[i]);
                continue;
            }
            var indent = lines[i][..(lines[i].Length - lines[i].TrimStart().Length)];
            var groups = new List<List<string[]>> { new() };
            var j = i + 1;
            var closed = false;
            var sawMid = false;
            for (; j < lines.Length; j++)
            {
                var u = lines[j].Trim();
                if (IsTableBottom(u)) { closed = true; break; }
                if (IsTableMid(u)) { groups.Add(new()); sawMid = true; continue; }
                if (!IsTableRow(u)) break;
                groups[^1].Add(u[1..^1].Split('│').Select(c => c.Trim()).ToArray());
            }
            if (!closed || groups[0].Count == 0)
            {
                output.Add(lines[i]);
                continue;
            }
            // Without separators every line is its own row; with them a group is one row.
            var rows = sawMid
                ? groups.Where(g => g.Count > 0).Select(g => Merge(g)).ToList()
                : groups[0];
            var cols = rows.Max(r => r.Length);
            string Row(string[] r) => indent + "| " + string.Join(" | ", Enumerable.Range(0, cols).Select(k => k < r.Length ? r[k].Replace("|", "\\|") : "")) + " |";
            if (output.Count > 0 && output[^1].Trim().Length > 0) output.Add("");
            output.Add(Row(rows[0]));
            output.Add(indent + "|" + string.Concat(Enumerable.Repeat("---|", cols)));
            foreach (var r in rows.Skip(1)) output.Add(Row(r));
            output.Add("");
            i = j;
        }
        return string.Join('\n', output);

        static string[] Merge(List<string[]> g)
        {
            var n = g.Max(r => r.Length);
            return Enumerable.Range(0, n)
                .Select(k => string.Join(' ', g.Select(r => k < r.Length ? r[k] : "").Where(s => s.Length > 0)))
                .ToArray();
        }
    }

    // ---- generic: shell prompts, command output, prose ----------------------------------

    // user@host:~/dir$ cmd   ·   (venv) user@host dir % cmd   ·   $ cmd   ·   ➜  dir git:(main) ✗ cmd   ·   ❯ cmd   ·   PS C:\> cmd
    [GeneratedRegex(@"^(?:\([\w.-]+\)\s+)?[\w.-]+@[\w.-]+(?::[^\s$#%]*|\s+\S+)?\s*[$#%]\s?(.*)$")] private static partial Regex UserHostPrompt();
    [GeneratedRegex(@"^[$%]\s?(.*)$")] private static partial Regex BarePrompt();
    [GeneratedRegex(@"^➜\s+\S+(?:\s+git:\([^)]*\))?(?:\s+✗)?\s?(.*)$")] private static partial Regex OhMyZshPrompt();
    [GeneratedRegex(@"^❯\s?(.*)$")] private static partial Regex StarshipPrompt();
    [GeneratedRegex(@"^PS [A-Za-z]:[^>]*>\s?(.*)$")] private static partial Regex PowerShellPrompt();
    [GeneratedRegex(@"^[>›]\s(\S.*)$")] private static partial Regex AgentPrompt();

    /// <summary>A shell prompt line: the command typed after it ("" for an idle prompt), else null.</summary>
    public static string? ShellCommand(string plain)
    {
        foreach (var re in new[] { UserHostPrompt(), OhMyZshPrompt(), StarshipPrompt(), PowerShellPrompt(), BarePrompt() })
        {
            var m = re.Match(plain);
            if (m.Success) return m.Groups[1].Value.Trim();
        }
        return null;
    }

    public static List<ScreenTurn> Generic(string raw)
    {
        var turns = new List<ScreenTurn>();
        var rawLines = raw.Replace("\r\n", "\n").Split('\n')
            .Select(l => LastCrSegment(l))
            .ToList();
        var anyShell = rawLines.Any(l => ShellCommand(StripAnsi(l).TrimEnd()) is { Length: > 0 });

        var buf = new List<string>();
        var mode = anyShell ? "output" : "prose"; // what the buffered lines are
        void Flush()
        {
            while (buf.Count > 0 && StripAnsi(buf[^1]).Trim().Length == 0) buf.RemoveAt(buf.Count - 1);
            while (buf.Count > 0 && StripAnsi(buf[0]).Trim().Length == 0) buf.RemoveAt(0);
            if (buf.Count > 0)
            {
                if (mode == "output")
                {
                    turns.Add(new ScreenTurn("assistant", "output", string.Join('\n', buf)));
                }
                else
                {
                    var md = ShapeMarkdown(string.Join('\n', buf.Select(StripAnsi)));
                    if (md.Trim().Length > 0) turns.Add(new ScreenTurn("assistant", "text", md));
                }
            }
            buf.Clear();
        }

        foreach (var line in rawLines)
        {
            var plain = StripAnsi(line).TrimEnd();
            if (IsChrome(plain)) continue;
            if (ShellCommand(plain) is { } cmd)
            {
                Flush();
                // An idle prompt with nothing typed is the shell waiting, not a turn.
                if (cmd.Length > 0) turns.Add(new ScreenTurn("user", "command", cmd));
                mode = "output";
                continue;
            }
            if (AgentPrompt().Match(plain) is { Success: true } ask)
            {
                Flush();
                turns.Add(new ScreenTurn("user", "text", ask.Groups[1].Value.Trim()));
                mode = "prose";
                continue;
            }
            buf.Add(line.TrimEnd());
        }
        Flush();
        return turns;
    }

    /// <summary>A spinner rewrites its line with \r; a terminal shows only the last segment.</summary>
    private static string LastCrSegment(string l)
    {
        var i = l.LastIndexOf('\r');
        return i < 0 ? l : l[(i + 1)..];
    }

    // ---- shapeTranscript, ported -------------------------------------------------------

    private const int RuleMin = 8, CodeRunMin = 2, ReflowMinFullLines = 3, ReflowRagged = 12, ReflowMaxRun = 60, ReflowMinWidth = 40, HardMinLines = 2;
    private static readonly HashSet<char> RuleChars = ['─', '━', '═', '-', '_', '=', '⎯', '‾', '▁', '·', '•'];

    [GeneratedRegex(@"[─-╿▀-▟]")] private static partial Regex BoxDrawing();
    [GeneratedRegex(@"^\s*\d+\s+[-+−±]?\s?")] private static partial Regex Gutter();
    [GeneratedRegex(@"^\s*[-+−]\s?\S")] private static partial Regex DiffMarker();
    [GeneratedRegex(@"^\s*\d+$")] private static partial Regex BareGutter();
    [GeneratedRegex(@"\S {2,}\S[\s\S]*?\S {2,}\S")] private static partial Regex Columns();
    [GeneratedRegex(@"^\s*[$#>]\s\S")] private static partial Regex ShellLike();
    [GeneratedRegex(@"\S*/\S+/\S+")] private static partial Regex Pathish();
    [GeneratedRegex(@"[{}\[\]()<>;=|$`~^*/\\]")] private static partial Regex CodePunct();
    [GeneratedRegex(@"^(?:[-*+•▪◦]\s|\d+[.)]\s|[>›❯»→#|])")] private static partial Regex BlockStart();
    [GeneratedRegex(@"^(\s*)[-*\u2022\u25aa\u25e6\u2013\u2014]\s+(\S.*)$")] private static partial Regex Bullet();

    internal static bool IsRule(string line)
    {
        var t = line.Trim();
        if (t.Length < RuleMin || !RuleChars.Contains(t[0])) return false;
        return t.All(c => c == t[0]);
    }

    private static bool IsTableLine(string line) => line.TrimStart().StartsWith('|') && line.TrimEnd().EndsWith('|');

    internal static bool IsCodeLike(string line)
    {
        if (line.Trim().Length == 0 || IsRule(line) || IsTableLine(line)) return false;
        if (BoxDrawing().IsMatch(line) || Gutter().IsMatch(line) || DiffMarker().IsMatch(line) || BareGutter().IsMatch(line)
            || ShellLike().IsMatch(line) || Pathish().IsMatch(line) || Columns().IsMatch(line)) return true;
        var t = line.Trim();
        return t.Length >= 8 && (double)CodePunct().Matches(t).Count / t.Length > 0.08;
    }

    private static string IndentOf(string s) => s[..(s.Length - s.TrimStart(' ', '\t').Length)];

    private static int DetectWrapColumn(List<string> all)
    {
        var lines = all.Where(l => l.Length > 0 && !IsCodeLike(l)).ToList();
        var max = lines.Count == 0 ? 0 : lines.Max(l => l.Length);
        if (max < ReflowMinWidth) return 0;
        return lines.Count(l => l.Length >= max - ReflowRagged) >= ReflowMinFullLines ? max : 0;
    }

    private static int DetectHardColumn(List<string> all)
    {
        var lines = all.Where(l => !IsRule(l)).ToList();
        var max = lines.Count == 0 ? 0 : lines.Max(l => l.Length);
        if (max < ReflowMinWidth) return 0;
        return lines.Count(l => l.Length == max) >= HardMinLines ? max : 0;
    }

    private static string Seam(string prev, string next) => prev.EndsWith(' ') || next.StartsWith(' ') ? " " : "";

    private static (List<string> Lines, List<int> Tail) UnwrapHardCuts(List<string> lines, int hardAt)
    {
        if (hardAt <= 0) return (lines, lines.Select(l => l.Length).ToList());
        var output = new List<string>();
        var tail = new List<int>();
        foreach (var line in lines)
        {
            if (output.Count > 0 && tail[^1] >= hardAt && !IsRule(output[^1]) && line.Length > 0 && !IsRule(line)
                && !BlockStart().IsMatch(line[IndentOf(line).Length..]))
            {
                output[^1] = output[^1] + Seam(output[^1], line) + line;
                tail[^1] = line.Length;
                continue;
            }
            output.Add(line);
            tail.Add(line.Length);
        }
        return (output, tail);
    }

    private static bool IsContinuation(string prev, string next, int wrapAt, int prevLen)
    {
        if (prev.Length == 0 || next.Length == 0 || IsRule(prev) || IsRule(next) || IsCodeLike(prev) || IsCodeLike(next)) return false;
        var delta = IndentOf(next).Length - IndentOf(prev).Length;
        if (delta is < 0 or > 4) return false;
        var body = next[IndentOf(next).Length..];
        if (BlockStart().IsMatch(body)) return false;
        var sp = body.IndexOf(' ');
        var firstWord = sp < 0 ? body : body[..sp];
        if (firstWord.Length == 0) return false;
        return prevLen + 1 + firstWord.Length > wrapAt;
    }

    /// <summary>
    /// shapeTranscript (herdr-expose transcriptText.ts) with markdown as its output: blank
    /// runs collapse to one, the pane's hard and soft wraps are undone, long rules become
    /// rules, code-shaped runs (gutters, diffs, paths, columns, punctuation) become fenced
    /// blocks that keep their columns, and bullets become a real list. Nothing is reworded.
    /// </summary>
    public static string ShapeMarkdown(string raw)
    {
        var rawLines = raw.Split('\n').Select(l => l.TrimEnd(' ', '\t')).ToList();
        var wrapAt = DetectWrapColumn(rawLines);
        var (lines, tail) = UnwrapHardCuts(rawLines, DetectHardColumn(rawLines));

        // kind: text | blank | rule | code | list
        var items = new List<(string Kind, List<string> Lines)>();
        var blankRun = 0;
        string? lastProse = null;
        var lastProseLen = 0;
        var joinRun = 0;

        void FlushBlanks()
        {
            if (blankRun == 0) return;
            if (items.Count > 0) items.Add(("blank", new()));
            blankRun = 0;
            lastProse = null;
            joinRun = 0;
        }

        for (var i = 0; i < lines.Count; i++)
        {
            var line = lines[i];
            if (line.Length == 0) { blankRun++; continue; }

            if (IsCodeLike(line))
            {
                var j = i;
                var run = new List<string>();
                while (j < lines.Count)
                {
                    if (IsCodeLike(lines[j])) { run.Add(lines[j]); j++; continue; }
                    if (lines[j].Length == 0 && j + 1 < lines.Count && IsCodeLike(lines[j + 1])) { run.Add(""); j++; continue; }
                    break;
                }
                if (run.Count >= CodeRunMin)
                {
                    FlushBlanks();
                    items.Add(("code", run));
                    lastProse = null;
                    joinRun = 0;
                    i = j - 1;
                    continue;
                }
            }

            FlushBlanks();
            if (IsRule(line))
            {
                items.Add(("rule", new()));
                lastProse = null;
                joinRun = 0;
                continue;
            }
            var bullet = Bullet().Match(line);
            if (bullet.Success)
            {
                if (items.Count > 0 && items[^1].Kind == "list") items[^1].Lines.Add(bullet.Groups[2].Value);
                else items.Add(("list", new() { bullet.Groups[2].Value }));
                lastProse = line;
                lastProseLen = tail[i];
                joinRun = 0;
                continue;
            }
            if (wrapAt > 0 && items.Count > 0 && items[^1].Kind == "list" && lastProse is not null && joinRun < ReflowMaxRun
                && IndentOf(line).Length > 0 && IsContinuation(lastProse, line, wrapAt, lastProseLen))
            {
                items[^1].Lines[^1] += " " + line.TrimStart();
                joinRun++;
                lastProse = line;
                lastProseLen = tail[i];
                continue;
            }
            if (wrapAt > 0 && items.Count > 0 && items[^1].Kind == "text" && lastProse is not null && joinRun < ReflowMaxRun
                && IsContinuation(lastProse, line, wrapAt, lastProseLen))
            {
                items[^1].Lines[^1] += " " + line.TrimStart();
                joinRun++;
            }
            else
            {
                items.Add(("text", new() { line }));
                joinRun = 0;
            }
            lastProse = line;
            lastProseLen = tail[i];
        }

        var sb = new StringBuilder();
        foreach (var (kind, body) in items)
        {
            switch (kind)
            {
                case "blank": sb.Append('\n'); break;
                case "rule": sb.Append("\n* * *\n\n"); break;
                case "list":
                    foreach (var l in body) sb.Append("- ").Append(l).Append('\n');
                    break;
                case "code":
                    var fence = body.Any(l => l.Contains("```")) ? "````" : "```";
                    sb.Append('\n').Append(fence).Append('\n');
                    foreach (var l in body) sb.Append(l).Append('\n');
                    sb.Append(fence).Append("\n\n");
                    break;
                default:
                    sb.Append(body[0].TrimStart()).Append('\n');
                    break;
            }
        }
        return sb.ToString().Trim('\n');
    }
}
