using System.Text.RegularExpressions;

namespace KlarkHub.Services;

/// <summary>
/// A slash command's output for agents whose structured log never records local commands
/// (Devin): read off the pane's screen as what is new since before the command was sent.
/// </summary>
public static class CommandOutput
{
    private static readonly Regex SlashCommand = new(@"^/\S+( .*)?$", RegexOptions.Compiled);
    private static readonly Regex PromptGlyph = new(@"^\s*[❭›>»$%#➜]\s*", RegexOptions.Compiled);

    // Devin chrome TranscriptClean does not know: the empty composer's hint and the model footer.
    private static readonly Regex DevinChrome = new(@"Ask Devin to build features|^\s*SWE-[\w.-]+\s+\w+\s*$|^\s*(Claude|GPT|Gemini)[\w .-]*\s+(Low|Medium|High)\s*$", RegexOptions.Compiled);

    public static bool IsSlashCommand(string? text) => text is not null && !text.Contains('\n') && SlashCommand.IsMatch(text.Trim());

    /// <summary>
    /// The lines on <paramref name="after"/> that are not on <paramref name="before"/> (counted, so a
    /// repeated line is new only as often as it grew), ANSI and agent chrome removed, the echo of
    /// <paramref name="command"/> dropped, trimmed. Empty when nothing new.
    /// </summary>
    public static string Extract(string before, string after, string command)
    {
        var seen = new Dictionary<string, int>();
        foreach (var l in Lines(TranscriptClean.Clean(before)))
        {
            seen[l] = seen.GetValueOrDefault(l) + 1;
        }
        var cmd = command.Trim();
        var fresh = new List<string>();
        foreach (var l in Lines(TranscriptClean.Clean(after)))
        {
            if (l.Trim().Length > 0 && seen.TryGetValue(l, out var n) && n > 0)
            {
                seen[l] = n - 1;
                continue;
            }
            if (l.Trim().Length == 0 && fresh.Count == 0) continue;
            var t = l.Trim();
            if (t == cmd || PromptGlyph.Replace(t, "") == cmd) continue;
            if (DevinChrome.IsMatch(l)) continue;
            fresh.Add(l);
        }
        return string.Join("\n", fresh).Trim('\n', ' ', '\t');
    }

    private static IEnumerable<string> Lines(string cleaned) => cleaned.Replace("\r\n", "\n").Split('\n').Select(l => l.TrimEnd());
}
