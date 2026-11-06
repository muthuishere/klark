using System.Text;
using System.Text.RegularExpressions;

namespace KlarkHub.Services;

/// <summary>
/// Turns a captured screen into text a phone can read (ADR 0011, D). Ported from
/// herdr-expose internal/core/clean.go: strip ANSI, drop the agent's own chrome
/// (rules, empty prompt boxes, context meters, mode banners, timing lines),
/// trim trailing whitespace, collapse runs of blank lines. A line that survives
/// is otherwise unchanged; when in doubt a line is content.
/// </summary>
public static partial class TranscriptClean
{
    private static readonly Regex[] Chrome =
    [
        // A rule: eight or more box-drawing, dash or underscore characters and nothing else.
        new(@"^[\s─-╿_=\-—–]{8,}$", RegexOptions.Compiled),
        // The empty input box: the TUI's own caret with nothing after it.
        new(@"^\s*[❯›]\s*$", RegexOptions.Compiled),
        // A context meter.
        new(@"ctx\s+[\d.]+[KM]?\s+\d+%", RegexOptions.Compiled | RegexOptions.IgnoreCase),
        // Mode banners.
        new(@"\b(auto mode|bypass permissions|accept edits)\b.*\(", RegexOptions.Compiled | RegexOptions.IgnoreCase),
        new(@"shift\+tab to cycle", RegexOptions.Compiled | RegexOptions.IgnoreCase),
        new(@"^\s*Image in clipboard", RegexOptions.Compiled | RegexOptions.IgnoreCase),
        // "✻ Sautéed for 1m 6s · done 10:10 PM" — matched by shape, not vocabulary.
        new(@"\bfor\s+(\d+h\s*)?(\d+m\s*)?\d+s\b.*·.*\bdone\b", RegexOptions.Compiled | RegexOptions.IgnoreCase),
        // The same line while running: "✽ Canoodling… (6m 39s · ↓ 28.1k tokens)".
        new(@"·\s*[↓v]?\s*[\d.]+k?\s*tokens", RegexOptions.Compiled | RegexOptions.IgnoreCase),
        new(@"^\s*\S?\s*\w+…\s*\(", RegexOptions.Compiled | RegexOptions.IgnoreCase),
        // A bare token tally on its own line (prose that mentions tokens survives).
        new(@"^\s*[\d.]+\s*k?\s*tokens\s*$", RegexOptions.Compiled | RegexOptions.IgnoreCase),
    ];

    [GeneratedRegex(@"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[@-Z\\-_]")]
    private static partial Regex Ansi();

    public static string StripAnsi(string text) => Ansi().Replace(text, "");

    public static string Clean(string text)
    {
        var output = new List<string>();
        var blankRun = 0;
        foreach (var raw in StripAnsi(text).Replace("\r\n", "\n").Replace('\r', '\n').Split('\n'))
        {
            var line = raw.TrimEnd(' ', '\t');
            if (line.Length > 0 && Chrome.Any(re => re.IsMatch(line))) continue;
            if (line.Length == 0)
            {
                blankRun++;
                continue;
            }
            if (blankRun > 0 && output.Count > 0) output.Add("");
            blankRun = 0;
            output.Add(line);
        }
        return string.Join('\n', output);
    }

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
