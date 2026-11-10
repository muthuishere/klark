using KlarkHub.Services;

namespace KlarkHub.Components.Pages;

/// <summary>
/// Which rows of a long chat are in the DOM. The window is a half-open range [Lo, Hi) of row
/// indexes; the browser (chat.js) decides it from the scroll position and the measured row
/// heights, and the page only clamps and keeps it sensible as rows arrive.
/// </summary>
public static class ChatWindow
{
    /// <summary>A chat this short is always drawn whole (no spacers, no windowing).</summary>
    public const int Always = 150;
    /// <summary>Rows drawn at first for a longer chat: the newest ones.</summary>
    public const int Tail = 60;
    /// <summary>Rows older than a hit that are drawn with it (and newer ones, twice that).</summary>
    public const int Around = 20;

    public static (int Lo, int Hi) Initial(int n) => n <= Always ? (0, n) : (n - Tail, n);

    /// <summary>Keep 0 &lt;= lo &lt;= hi &lt;= n.</summary>
    public static (int Lo, int Hi) Clamp(int lo, int hi, int n)
    {
        hi = Math.Clamp(hi, 0, n);
        lo = Math.Clamp(lo, 0, hi);
        return (lo, hi);
    }

    /// <summary>
    /// The rows changed from <paramref name="oldN"/> to <paramref name="n"/>. A window that reached
    /// the newest row keeps reaching it, so streaming text stays drawn.
    /// </summary>
    public static (int Lo, int Hi) Follow(int lo, int hi, int oldN, int n)
    {
        var atEnd = hi >= oldN;
        if (n <= Always && lo == 0) return (0, n);
        return Clamp(lo, atEnd ? n : hi, n);
    }

    /// <summary>The window that draws row <paramref name="index"/> with some context.</summary>
    public static (int Lo, int Hi) AroundRow(int index, int n) =>
        Clamp(index - Around, index + 2 * Around, n);

    /// <summary>The fallback button: grow the window by <paramref name="page"/> older rows.</summary>
    public static (int Lo, int Hi) Earlier(int lo, int hi, int page, int n) => Clamp(lo - page, hi, n);

    /// <summary>Pixel height guess for a row not measured yet; real heights replace it once drawn.</summary>
    public static int Estimate(ChatRowModel row)
    {
        static int Lines(string? s, int width)
        {
            if (string.IsNullOrEmpty(s)) return 0;
            var lines = 0;
            foreach (var l in s.Split('\n')) lines += Math.Max(1, (l.Length + width - 1) / width);
            return lines;
        }
        switch (row.Kind)
        {
            case "user":
                return 20 + 14 + 24 * Math.Min(Lines(row.Items[0].Text, 70), 40);
            case "text":
                var h = 8;
                var md = row.Items[0].Md;
                if (md is null || md.Count == 0) return h + 24 * Math.Max(1, Lines(row.Items[0].Text, 90));
                foreach (var seg in md)
                {
                    h += seg switch
                    {
                        MdCode c => 50 + 20 * Lines(c.Code, 100),
                        MdHtml m => 14 + 26 * Math.Max(1, Lines(System.Text.RegularExpressions.Regex.Replace(m.Html, "<[^>]+>", ""), 90)),
                        MdAnsi a => 20 + 19 * Math.Min(a.Lines, 27),
                        _ => 0,
                    };
                }
                return h;
            case "block":
                var lines = row.Items.Select(i => i.Md?.OfType<MdAnsi>().FirstOrDefault()?.Lines ?? 0).Max();
                return 42 + (lines is > 0 and <= 40 ? 19 * lines + 20 : 0);
            default:
                return 36;
        }
    }
}
