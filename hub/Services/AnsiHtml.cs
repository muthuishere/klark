using System.Net;
using System.Text;
using System.Text.RegularExpressions;

namespace KlarkHub.Services;

/// <summary>
/// ANSI SGR to escaped HTML spans, for command output in the chat (ADR 0011, F). Behaviour
/// ported from herdr-expose web/src/ansi/ansiToHtml.ts: colours (16 as classes so the theme
/// picks them, 256 and truecolor as inline rgb), bold, dim, italic, underline, inverse; every
/// other escape is dropped. Not an emulator: the input is an already laid-out screen.
/// </summary>
public static partial class AnsiHtml
{
    [GeneratedRegex(@"\x1b\[([0-9;:]*)m|\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]")]
    private static partial Regex Escape();

    private sealed record State(int Fg = -1, int Bg = -1, string? FgRgb = null, string? BgRgb = null,
        bool Bold = false, bool Dim = false, bool Italic = false, bool Under = false, bool Inverse = false)
    {
        public bool Plain => this == new State();
    }

    public static bool HasAnsi(string s) => s.Contains('\u001b');

    public static string Render(string text)
    {
        var sb = new StringBuilder();
        var st = new State();
        var open = false;
        var pos = 0;
        void Text(string s)
        {
            if (s.Length == 0) return;
            if (!open && !st.Plain) { sb.Append(Open(st)); open = true; }
            sb.Append(WebUtility.HtmlEncode(s));
        }
        foreach (Match m in Escape().Matches(text))
        {
            Text(text[pos..m.Index]);
            pos = m.Index + m.Length;
            if (!m.Groups[1].Success) continue; // not SGR: dropped
            var next = Apply(st, m.Groups[1].Value);
            if (next == st) continue;
            if (open) { sb.Append("</span>"); open = false; }
            st = next;
        }
        Text(text[pos..]);
        if (open) sb.Append("</span>");
        return sb.ToString();
    }

    private static State Apply(State s, string codes)
    {
        var p = codes.Length == 0 ? new[] { 0 } : codes.Split(';', ':').Select(x => int.TryParse(x, out var n) ? n : 0).ToArray();
        for (var i = 0; i < p.Length; i++)
        {
            var c = p[i];
            switch (c)
            {
                case 0: s = new State(); break;
                case 1: s = s with { Bold = true }; break;
                case 2: s = s with { Dim = true }; break;
                case 3: s = s with { Italic = true }; break;
                case 4: s = s with { Under = true }; break;
                case 7: s = s with { Inverse = true }; break;
                case 22: s = s with { Bold = false, Dim = false }; break;
                case 23: s = s with { Italic = false }; break;
                case 24: s = s with { Under = false }; break;
                case 27: s = s with { Inverse = false }; break;
                case >= 30 and <= 37: s = s with { Fg = c - 30, FgRgb = null }; break;
                case 39: s = s with { Fg = -1, FgRgb = null }; break;
                case >= 40 and <= 47: s = s with { Bg = c - 40, BgRgb = null }; break;
                case 49: s = s with { Bg = -1, BgRgb = null }; break;
                case >= 90 and <= 97: s = s with { Fg = c - 90 + 8, FgRgb = null }; break;
                case >= 100 and <= 107: s = s with { Bg = c - 100 + 8, BgRgb = null }; break;
                case 38 or 48:
                    string? rgb = null;
                    var idx = -1;
                    if (i + 2 < p.Length && p[i + 1] == 5) { idx = p[i + 2]; i += 2; }
                    else if (i + 4 < p.Length && p[i + 1] == 2) { rgb = $"rgb({Clamp(p[i + 2])},{Clamp(p[i + 3])},{Clamp(p[i + 4])})"; i += 4; }
                    if (idx is >= 0 and < 16) s = c == 38 ? s with { Fg = idx, FgRgb = null } : s with { Bg = idx, BgRgb = null };
                    else if (idx >= 16) rgb = Xterm256(idx);
                    if (rgb is not null) s = c == 38 ? s with { FgRgb = rgb, Fg = -1 } : s with { BgRgb = rgb, Bg = -1 };
                    break;
            }
        }
        return s;
    }

    private static int Clamp(int v) => Math.Clamp(v, 0, 255);

    private static string Xterm256(int n)
    {
        n = Math.Clamp(n, 16, 255);
        if (n >= 232) { var g = 8 + (n - 232) * 10; return $"rgb({g},{g},{g})"; }
        n -= 16;
        int L(int v) => v == 0 ? 0 : 55 + v * 40;
        return $"rgb({L(n / 36)},{L(n / 6 % 6)},{L(n % 6)})";
    }

    private static string Open(State s)
    {
        var cls = new List<string>();
        var style = new List<string>();
        var (fg, bg, fgRgb, bgRgb) = s.Inverse ? (s.Bg < 0 && s.BgRgb is null ? 16 : s.Bg, s.Fg < 0 && s.FgRgb is null ? 17 : s.Fg, s.BgRgb, s.FgRgb) : (s.Fg, s.Bg, s.FgRgb, s.BgRgb);
        if (fgRgb is not null) style.Add("color:" + fgRgb); else if (fg >= 0) cls.Add("af" + fg);
        if (bgRgb is not null) style.Add("background:" + bgRgb); else if (bg >= 0) cls.Add("ab" + bg);
        if (s.Bold) cls.Add("a-b");
        if (s.Dim) cls.Add("a-d");
        if (s.Italic) cls.Add("a-i");
        if (s.Under) cls.Add("a-u");
        var sb = new StringBuilder("<span");
        if (cls.Count > 0) sb.Append(" class=\"").Append(string.Join(' ', cls)).Append('"');
        if (style.Count > 0) sb.Append(" style=\"").Append(string.Join(';', style)).Append('"');
        return sb.Append('>').ToString();
    }
}
