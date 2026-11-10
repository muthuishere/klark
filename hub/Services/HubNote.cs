using System.Text.RegularExpressions;

namespace KlarkHub.Services;

/// <summary>
/// A short instruction added to what a person sends from the hub (never to slash commands),
/// so the agent names files it creates or changes by absolute path, which the hub turns
/// into links that open a preview. It is a single parenthesised line, so it travels as an
/// ordinary part of the prompt: the terminal shows it, the hub's chat hides it.
/// Set Hub:AgentNote (env Hub__AgentNote) to change the wording; empty turns it off.
/// </summary>
public static partial class HubNote
{
    public const string DefaultBody = "when you create or change a file, state its absolute path so it can be opened from the hub.";

    [GeneratedRegex(@"\s*\(klark hub: [^)]*\)\s*$")]
    private static partial Regex Tail();

    /// <summary>The text with the note added. Unchanged when the note is empty, the text is empty or is a slash command.</summary>
    public static string Append(string text, string? body)
    {
        if (string.IsNullOrWhiteSpace(body) || string.IsNullOrWhiteSpace(text) || text.TrimStart().StartsWith('/')) return text;
        if (Tail().IsMatch(text)) return text;
        return $"{text} (klark hub: {body.Trim().Replace('(', '[').Replace(')', ']').ReplaceLineEndings(" ")})";
    }

    /// <summary>The text as the person wrote it: the note, if it is at the end, removed.</summary>
    public static string Strip(string? text) => string.IsNullOrEmpty(text) ? "" : Tail().Replace(text, "");
}
