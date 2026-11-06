using Markdig;
using Markdig.Renderers;
using Markdig.Syntax;
using Markdig.Syntax.Inlines;

namespace KlarkHub.Services;

/// <summary>A rendered piece of a message: HTML, or a code block drawn with a label and a copy button.</summary>
public abstract record MdSegment;
public sealed record MdHtml(string Html) : MdSegment;
public sealed record MdCode(string Lang, string Code) : MdSegment;

/// <summary>
/// Markdown for the chat view (ADR 0011, D). Raw HTML is disabled; links keep
/// only http, https and mailto; images are never loaded (they become their alt
/// text), so a transcript cannot make a viewer's browser fetch anything.
/// </summary>
public static class ChatMarkdown
{
    private static readonly MarkdownPipeline Pipeline = new MarkdownPipelineBuilder()
        .DisableHtml()
        .UsePipeTables()
        .UseEmphasisExtras()
        .UseTaskLists()
        .UseAutoLinks()
        .Build();

    /// <summary>Text from a screen keeps its line breaks.</summary>
    private static readonly MarkdownPipeline ScreenPipeline = new MarkdownPipelineBuilder()
        .DisableHtml()
        .UsePipeTables()
        .UseAutoLinks()
        .UseSoftlineBreakAsHardlineBreak()
        .Build();

    public static bool SafeUrl(string? url)
    {
        if (string.IsNullOrWhiteSpace(url)) return false;
        return Uri.TryCreate(url.Trim(), UriKind.Absolute, out var u) && u.Scheme is "http" or "https" or "mailto";
    }

    public static IReadOnlyList<MdSegment> Render(string markdown, bool fromScreen = false)
    {
        var pipeline = fromScreen ? ScreenPipeline : Pipeline;
        var doc = Markdown.Parse(markdown, pipeline);
        foreach (var link in doc.Descendants<LinkInline>().ToList())
        {
            if (!link.IsImage && SafeUrl(link.Url)) continue;
            // Unsafe or image: keep what it said, drop where it pointed.
            var text = string.Concat(link.Descendants<LiteralInline>().Select(l => l.Content.ToString()));
            if (text.Length == 0) text = link.Url ?? "";
            link.ReplaceBy(new LiteralInline(text), copyChildren: false);
        }
        foreach (var auto in doc.Descendants<AutolinkInline>().ToList())
        {
            if (!SafeUrl(auto.IsEmail ? "mailto:" + auto.Url : auto.Url))
                auto.ReplaceBy(new LiteralInline(auto.Url), copyChildren: false);
        }

        var segments = new List<MdSegment>();
        var pending = new List<Block>();
        void Flush()
        {
            if (pending.Count == 0) return;
            using var w = new StringWriter();
            var r = new HtmlRenderer(w);
            pipeline.Setup(r);
            foreach (var b in pending) r.Render(b);
            w.Flush();
            segments.Add(new MdHtml(w.ToString()));
            pending.Clear();
        }
        foreach (var block in doc)
        {
            if (block is CodeBlock code)
            {
                Flush();
                var lang = code is FencedCodeBlock f ? (f.Info ?? "").Trim() : "";
                segments.Add(new MdCode(lang, code.Lines.ToString()));
            }
            else
            {
                pending.Add(block);
            }
        }
        Flush();
        return segments;
    }
}
