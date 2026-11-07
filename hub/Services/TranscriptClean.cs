using System.Text;
using System.Text.RegularExpressions;

namespace KlarkHub.Services;

/// <summary>
/// Turns a captured screen into text a phone can read (ADR 0011, D and E). Ported from
/// herdr-expose internal/core/clean.go and extended: strip ANSI, drop the agent's own
/// chrome (rules, prompt-box borders, context meters, mode banners, key hints, spinner
/// and timing lines, version banners), trim trailing whitespace, collapse blank runs.
/// A line that survives is otherwise unchanged; when in doubt a line is content.
/// Inside a fenced code block nothing is treated as chrome.
/// </summary>
public static partial class TranscriptClean
{
    private const RegexOptions Opt = RegexOptions.Compiled | RegexOptions.IgnoreCase | RegexOptions.CultureInvariant;

    private static readonly Regex[] Chrome =
    [
        // A rule: eight or more horizontal line, dash or underscore characters (prompt-box
        // corners allowed) and nothing else. Table borders (┌┬┐├┼┤) are not rules.
        new(@"^[\s─━═┄┅┈┉╌╍╴╶╸╺_=\-—–╭╮╰╯]{8,}$", Opt),
        // The empty input box: the TUI's caret with nothing after it. ASCII '>' alone
        // too: an empty quote says nothing.
        new(@"^\s*[❯›>▌]\s*$", Opt),
        // Context meters.
        new(@"ctx\s+[\d.]+[KM]?\s+\d+%", Opt),
        new(@"context left until auto-?compact", Opt),
        new(@"\b\d+%\s+context\s+(left|remaining|used)\b", Opt),
        new(@"^\s*context\s*(remaining|left|used|window)?\s*:?\s*[\d.]+\s*[%kK]", Opt),
        new(@"\bcontext (remaining|low)\b.*\d", Opt),
        // Mode banners and key hints.
        new(@"\b(auto mode|bypass permissions|accept edits|plan mode|auto-accept edits)\b\s*(on|off)?\b.*(\(|$)", Opt),
        new(@"shift\+tab to (cycle|toggle)", Opt),
        new(@"^\s*[⏵⏸▸]{1,2}\s", Opt),
        new(@"^\s*\?\s*for shortcuts", Opt),
        new(@"\besc to (interrupt|cancel)\b", Opt),
        new(@"\bctrl\+[a-z] to (expand|collapse|exit|quit|show)\b", Opt),
        new(@"\b(press )?ctrl-?\+?c again to (exit|quit)\b", Opt),
        new(@"^\s*⏎ send\b", Opt),
        new(@"^\s*(⌃|ctrl\+)j newline\b", Opt),
        new(@"^\s*Image in clipboard", Opt),
        new(@"^\s*※\s*Tip:", Opt),
        // "✻ Sautéed for 1m 6s · done 10:10 PM" — matched by shape, not vocabulary.
        new(@"\bfor\s+(\d+h\s*)?(\d+m\s*)?\d+s\b.*·.*\bdone\b", Opt),
        // The same line while running: "✽ Canoodling… (6m 39s · ↓ 28.1k tokens)".
        new(@"·\s*[↓↑v^]?\s*[\d.]+k?\s*tokens", Opt),
        new(@"^\s*\S?\s*\w+(…|\.\.\.)\s*\(", Opt),
        // A bare spinner word: "· Cogitating…", "✶ Thinking…".
        new(@"^\s*[·✻✽✶✳✢✦*•⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]\s*\w+(…|\.\.\.)\s*$", Opt),
        // Token and cost tallies on their own line (prose that mentions tokens survives).
        new(@"^\s*[↑↓]?\s*[\d.,]+\s*k?\s*tokens(\s*[·|].*)?\s*$", Opt),
        new(@"^\s*(total\s+)?cost:\s*\$[\d.]+", Opt),
        new(@"^\s*total (duration|code changes)\b.*:", Opt),
        new(@"^\s*token usage:\s", Opt),
        // Version and welcome banners (Claude Code, Codex, Devin).
        new(@"^\s*[✻*>_]*\s*Welcome to (Claude Code|Devin|Codex)", Opt),
        new(@"^\s*(Claude Code|OpenAI Codex|Devin( CLI)?)\s*\(?v?\d+\.\d+", Opt),
        new(@"^\s*>_\s*OpenAI Codex", Opt),
        new(@"^\s*/help for help", Opt),
        new(@"^\s*(cwd|directory):\s+[~/]\S*\s*$", Opt),
        new(@"/model to change", Opt),
        new(@"^\s*To get started, describe a task", Opt),
    ];

    /// <summary>Lines inside a box that mean the box is a banner, not a dialog.</summary>
    private static readonly Regex BannerBox = new(@"Welcome to|/help for help|OpenAI Codex|Claude Code v?\d|Devin( CLI)? v?\d|cwd:\s", Opt);

    [GeneratedRegex(@"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[@-Z\\-_]")]
    private static partial Regex Ansi();

    /// <summary>"(ctrl+r to expand)" after real text: the hint goes, the text stays.</summary>
    [GeneratedRegex(@"\s*\((ctrl|⌃)\+[a-z] to (expand|collapse|see all)\)", RegexOptions.IgnoreCase)]
    private static partial Regex ExpandHint();

    public static string StripAnsi(string text) => Ansi().Replace(text, "");

    internal static bool IsChrome(string line) => line.Length > 0 && Chrome.Any(re => re.IsMatch(line));

    private static bool IsBoxTop(string t) => t.Length > 0 && t[0] is '╭' or '┌' or '╔' && t.Count(c => c is '─' or '━' or '═') >= 3 && !t.Contains('┬');
    private static bool IsBoxBottom(string t) => t.Length > 0 && t[0] is '╰' or '└' or '╚' && t.Count(c => c is '─' or '━' or '═') >= 3 && !t.Contains('┴');

    /// <summary>"│ > hello  │" → " > hello": the inside of a box line, or null when it is not one.</summary>
    private static string? BoxInner(string line)
    {
        var t = line.Trim();
        if (t.Length < 2 || t[0] is not ('│' or '┃' or '║')) return null;
        var end = t[^1] is '│' or '┃' or '║' ? t.Length - 1 : t.Length;
        var inner = t[1..end];
        // A table row has more separators; it is content, not a box.
        if (inner.Contains('│')) return null;
        return inner.TrimEnd();
    }

    public static string Clean(string text)
    {
        var output = new List<string>();
        var blankRun = 0;
        var fenced = false;
        List<string>? box = null;

        void Emit(string line)
        {
            if (line.Length == 0) { blankRun++; return; }
            if (blankRun > 0 && output.Count > 0) output.Add("");
            blankRun = 0;
            output.Add(line);
        }

        void FlushBox()
        {
            if (box is null) return;
            var lines = box;
            box = null;
            var inner = lines.Select(l => l.Trim()).Where(l => l.Length > 0).ToList();
            if (inner.Count == 0 || inner.Any(l => BannerBox.IsMatch(l))) return;
            // The input box: a caret, then what is being typed (a sent prompt stays: it is content).
            if (inner.All(l => IsChrome(l))) return;
            foreach (var l in lines)
            {
                var s = l.StartsWith(' ') ? l[1..] : l;
                if (!IsChrome(s)) Emit(s.TrimEnd());
            }
        }

        foreach (var raw in StripAnsi(text).Replace("\r\n", "\n").Replace('\r', '\n').Split('\n'))
        {
            var line = raw.TrimEnd(' ', '\t', ' ');
            if (fenced)
            {
                // Inside a code block every line, blank or not, is content.
                if (line.TrimStart().StartsWith("```")) fenced = false;
                if (blankRun > 0 && output.Count > 0) output.Add("");
                blankRun = 0;
                output.Add(line);
                continue;
            }
            line = ExpandHint().Replace(line, "");
            var t = line.Trim();
            if (box is not null)
            {
                if (IsBoxBottom(t)) { FlushBox(); continue; }
                if (BoxInner(line) is { } inside) { box.Add(inside); continue; }
                FlushBox();
            }
            if (IsBoxTop(t))
            {
                box = new List<string>();
                continue;
            }
            if (IsBoxBottom(t)) continue;
            if (BoxInner(line) is { } lone)
            {
                // A box line without its top (scrolled off): judge it on its own.
                var s = lone.StartsWith(' ') ? lone[1..] : lone;
                if (s.Trim().Length == 0 || IsChrome(s) || BannerBox.IsMatch(s)) continue;
                Emit(s.TrimEnd());
                continue;
            }
            if (IsChrome(line)) continue;
            if (t.StartsWith("```")) fenced = true;
            Emit(line);
        }
        FlushBox();
        return string.Join('\n', output);
    }

    /// <summary>One turn read off a cleaned screen.</summary>
    public sealed record ScreenTurn(string Role, string Kind, string Text, string? Tool = null, string Summary = "");

    [GeneratedRegex(@"^([\p{L}_][\w.:-]*)\((.*)\)\s*$")]
    private static partial Regex ToolHeader();

    /// <summary>
    /// Split a cleaned screen into turns: "> " / "❯ " / "› " at column 0 is the person,
    /// "⏺ Tool(args)" a tool call, "⎿ " (or "└ ") its result, "⏺ " / "• " other text the agent.
    /// Text before any marker belongs to the agent. Nothing is reworded.
    /// </summary>
    public static List<ScreenTurn> Turns(string cleaned) => Turns(cleaned, ">❯›", "⏺●•", "⎿└");

    /// <summary>Turns with one agent's own marker set (see <see cref="ScreenTransformers"/>).</summary>
    public static List<ScreenTurn> Turns(string cleaned, string userMarks, string agentMarks, string resultMarks)
    {
        var turns = new List<ScreenTurn>();
        string role = "assistant", kind = "text";
        string? tool = null;
        var summary = "";
        var buf = new List<string>();
        var fenced = false;

        void Flush()
        {
            while (buf.Count > 0 && buf[^1].Length == 0) buf.RemoveAt(buf.Count - 1);
            var body = string.Join('\n', buf).Trim('\n');
            if (body.Trim().Length > 0 || kind == "tool_use")
            {
                // Consecutive agent text with no marker between merges into one block.
                turns.Add(new ScreenTurn(role, kind, body, tool, summary));
            }
            buf.Clear();
            role = "assistant"; kind = "text"; tool = null; summary = "";
        }

        foreach (var line in cleaned.Split('\n'))
        {
            if (fenced)
            {
                if (line.TrimStart().StartsWith("```")) fenced = false;
                buf.Add(Dedent(line));
                continue;
            }
            if (Marker(line, userMarks, agentMarks, resultMarks, out var markerKind, out var rest))
            {
                Flush();
                switch (markerKind)
                {
                    case "user":
                        role = "user";
                        break;
                    case "result":
                        kind = "tool_result";
                        break;
                    default:
                        var m = ToolHeader().Match(rest);
                        if (m.Success && rest.Length < 200)
                        {
                            kind = "tool_use";
                            tool = m.Groups[1].Value;
                            summary = m.Groups[2].Value.Trim();
                            if (summary.Length > 70) summary = summary[..67] + "…";
                            turns.Add(new ScreenTurn("assistant", "tool_use", "", tool, summary));
                            kind = "text"; tool = null; summary = "";
                            continue;
                        }
                        break;
                }
                buf.Add(rest);
                if (rest.TrimStart().StartsWith("```")) fenced = true;
                continue;
            }
            var d = kind == "tool_result" ? line.TrimStart() : Dedent(line);
            if (d.TrimStart().StartsWith("```")) fenced = true;
            buf.Add(d);
        }
        Flush();

        // A result with no call before it still belongs to the agent; drop empty tool rows only.
        return turns;
    }

    private static string Dedent(string line) => line.StartsWith("  ") ? line[2..] : line;

    private static bool Marker(string line, string userMarks, string agentMarks, string resultMarks, out string kind, out string rest)
    {
        kind = rest = "";
        if (line.Length < 2) return false;
        var c = line[0];
        if (userMarks.Contains(c) && line[1] == ' ')
        {
            kind = "user";
            rest = line[2..].Trim();
            return rest.Length > 0;
        }
        if (agentMarks.Contains(c) && line[1] == ' ')
        {
            kind = "agent";
            rest = line[2..];
            return true;
        }
        var t = line.TrimStart();
        if (t.Length > 1 && resultMarks.Contains(t[0]) && t[1] == ' ' && line.Length - t.Length <= 6)
        {
            kind = "result";
            rest = t[2..].Trim();
            return true;
        }
        return false;
    }

    // ---- structured (jsonl/db) text --------------------------------------------------

    [GeneratedRegex(@"<(system-reminder|local-command-stdout|local-command-stderr|command-message|bash-stdout|bash-stderr|user-prompt-submit-hook)>.*?</\1>", RegexOptions.Singleline)]
    private static partial Regex Dropped();

    [GeneratedRegex(@"<command-name>\s*(.*?)\s*</command-name>", RegexOptions.Singleline)]
    private static partial Regex CommandName();

    [GeneratedRegex(@"<command-args>\s*(.*?)\s*</command-args>", RegexOptions.Singleline)]
    private static partial Regex CommandArgs();

    [GeneratedRegex(@"<bash-input>\s*(.*?)\s*</bash-input>", RegexOptions.Singleline)]
    private static partial Regex BashInput();

    [GeneratedRegex(@"^Caveat: The messages below were generated by the user while running local commands.*$", RegexOptions.Multiline)]
    private static partial Regex Caveat();

    [GeneratedRegex(@"</?(system-reminder|local-command-stdout|local-command-stderr|command-name|command-args|command-message)>")]
    private static partial Regex StrayTag();

    /// <summary>
    /// The text of a structured transcript item as a person should see it: harness wrappers
    /// (system reminders, local command output) removed, "&lt;command-name&gt;/x&lt;/command-name&gt;"
    /// shown as "/x args". Empty means the item has nothing to show.
    /// </summary>
    public static string CleanStructured(string? text)
    {
        if (string.IsNullOrEmpty(text)) return "";
        var command = CommandName().Match(text);
        if (command.Success)
        {
            var args = CommandArgs().Match(text);
            return (command.Groups[1].Value + (args.Success && args.Groups[1].Value.Length > 0 ? " " + args.Groups[1].Value : "")).Trim();
        }
        var bash = BashInput().Match(text);
        if (bash.Success) return "! " + bash.Groups[1].Value;
        var s = Dropped().Replace(text, "");
        s = Caveat().Replace(s, "");
        s = StrayTag().Replace(s, "");
        return s.Trim();
    }

    // ---- blocked prompts ---------------------------------------------------------------

    /// <summary>A question an agent is blocked on, and the quick replies a screen offers.</summary>
    public sealed record BlockedPrompt(string Question, IReadOnlyList<(string Label, string Send, bool Enter)> Options);

    [GeneratedRegex(@"^\s*(?:[❯>]\s*)?(\d{1,2})[.)]\s+(.{1,80})$")]
    private static partial Regex NumberedOption();

    [GeneratedRegex(@"\((?:y/n|yes/no)\)|\[(?:y/n|Y/n|y/N)\]", RegexOptions.IgnoreCase)]
    private static partial Regex YesNo();

    /// <summary>
    /// Read the tail of a cleaned screen for a question and its numbered or
    /// yes/no options. Nothing is guessed: no options found means no buttons.
    /// </summary>
    public static BlockedPrompt Blocked(string cleaned)
    {
        var lines = cleaned.Split('\n').TakeLast(30).ToList();
        var options = new List<(string, string, bool)>();
        var firstOption = -1;
        for (var i = 0; i < lines.Count; i++)
        {
            var m = NumberedOption().Match(lines[i]);
            if (!m.Success) continue;
            var n = m.Groups[1].Value;
            if (options.Any(o => o.Item2 == n)) continue;
            if (firstOption < 0) firstOption = i;
            options.Add(($"{n}. {m.Groups[2].Value.Trim()}", n, false));
        }
        string question;
        if (firstOption >= 0)
        {
            question = string.Join('\n', lines.Take(firstOption).TakeLast(8)).Trim();
        }
        else
        {
            question = string.Join('\n', lines.TakeLast(8)).Trim();
            if (lines.TakeLast(4).Any(l => YesNo().IsMatch(l)))
            {
                options.Add(("Yes", "y", true));
                options.Add(("No", "n", true));
            }
        }
        var sb = new StringBuilder(question);
        return new BlockedPrompt(sb.ToString(), options);
    }
}
