namespace KlarkHub.Components;

/// <summary>Inline SVG icons (stroke = currentColor), so the chat needs no icon font or image fetch.</summary>
public static class Icons
{
    private const string Open = "<svg viewBox=\"0 0 24 24\" width=\"20\" height=\"20\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\">";
    public const string Sidebar = Open + "<rect x=\"3\" y=\"4\" width=\"18\" height=\"16\" rx=\"3\"/><path d=\"M9 4v16\"/></svg>";
    public const string Terminal = Open + "<rect x=\"3\" y=\"4\" width=\"18\" height=\"16\" rx=\"3\"/><path d=\"m7 9 3 3-3 3M12 15h5\"/></svg>";
    public const string Chat = Open + "<path d=\"M4 5h16v11H9l-5 4z\"/></svg>";
    public const string More = Open + "<circle cx=\"5\" cy=\"12\" r=\"1\"/><circle cx=\"12\" cy=\"12\" r=\"1\"/><circle cx=\"19\" cy=\"12\" r=\"1\"/></svg>";
    public const string Plus = Open + "<path d=\"M12 5v14M5 12h14\"/></svg>";
    public const string Up = "<svg viewBox=\"0 0 24 24\" width=\"18\" height=\"18\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2.2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><path d=\"M12 19V5M5 12l7-7 7 7\"/></svg>";
    public const string Back = Open + "<path d=\"m15 18-6-6 6-6\"/></svg>";
    public const string File = Open + "<path d=\"M14 3H6a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8z\"/><path d=\"M14 3v5h5\"/></svg>";
    public const string Download = Open + "<path d=\"M12 4v11m-5-5 5 5 5-5M5 20h14\"/></svg>";
    public const string Camera = Open + "<path d=\"M4 8h3l2-3h6l2 3h3v11H4z\"/><circle cx=\"12\" cy=\"13\" r=\"3.5\"/></svg>";
    public const string Search = Open + "<circle cx=\"11\" cy=\"11\" r=\"6\"/><path d=\"m20 20-4.5-4.5\"/></svg>";
}
