using System.Net;
using System.Text;
using System.Text.RegularExpressions;

namespace KlarkHub.Services;

/// <summary>
/// File paths in chat text become links to the file viewer (ADR 0011, G): absolute, "~/"
/// and relative paths, with an optional ":line". The hub checks a path here before it ever
/// asks a machine (the machine checks again, see klark-link files.rs).
/// </summary>
public static partial class FileLinks
{
    private const string Ext = "rs|cs|razor|go|py|js|mjs|cjs|ts|tsx|jsx|java|kt|kts|swift|rb|php|c|h|cc|cpp|hpp|m|mm|scala|clj|ex|exs|erl|hs|lua|pl|r|dart|zig|nim|sh|bash|zsh|fish|ps1|sql|md|markdown|mdx|txt|log|json|jsonc|yaml|yml|toml|ini|cfg|conf|xml|html|htm|css|scss|less|svg|png|jpe?g|gif|webp|pdf|csv|tsv|diff|patch|lock|proto|graphql|gradle|tf|hcl|vue|svelte|astro|ipynb|sln|csproj|fsproj|props|targets|mod|sum|dockerfile";

    // A path: optional ~/ ./ ../ or /, segments of word characters, and (unless it has two
    // slashes or is absolute) a known extension. Then an optional :line[:col].
    [GeneratedRegex(@"(?<![\w/.~@:-])((?:~/|\.{1,2}/|/)?(?:[\w@+-][\w.@+-]*/)+[\w@+-][\w.@+-]*|[\w@+-][\w.@+-]*\.(?:" + Ext + @"))(?::(\d{1,6})(?::\d{1,4})?)?(?![\w/@+-]|\.\w)", RegexOptions.IgnoreCase)]
    private static partial Regex PathRe();

    [GeneratedRegex(@"\.(" + Ext + @")$", RegexOptions.IgnoreCase)]
    private static partial Regex KnownExt();

    [GeneratedRegex(@"^/(Users|home|tmp|private|var|etc|opt|usr|srv|mnt|Volumes|workspace|workspaces|root|data|app|code|src)/")]
    private static partial Regex FsRoot();

    public sealed record Found(int Index, int Length, string Path, int? Line);

    /// <summary>The file references in a run of plain text.</summary>
    public static List<Found> Find(string text)
    {
        var list = new List<Found>();
        foreach (Match m in PathRe().Matches(text))
        {
            var p = m.Groups[1].Value;
            var abs = p.StartsWith('/') || p.StartsWith("~/");
            var slashes = p.Count(c => c == '/');
            if (p.Contains("//") || p.EndsWith('.')) continue;
            // "and/or", "w/o": a relative path needs a known extension or two separators.
            if (!abs && !KnownExt().IsMatch(p) && slashes < 2) continue;
            if (abs && slashes < 2 && !KnownExt().IsMatch(p)) continue; // "/tmp" alone, "/s"
            // "/agent/events" is a URL route, not a file: an absolute path without a known
            // extension must start at a real filesystem root.
            if (p.StartsWith('/') && !KnownExt().IsMatch(p) && !FsRoot().IsMatch(p)) continue;
            if (p.StartsWith("http") || p.Contains("..."))continue;
            int? line = m.Groups[2].Success ? int.Parse(m.Groups[2].Value) : null;
            list.Add(new Found(m.Index, m.Length, p, line));
        }
        return list;
    }

    /// <summary>The viewer link for a path in a pane.</summary>
    public static string Href(string machine, string pane, string path, int? line) =>
        $"/m/{Uri.EscapeDataString(machine)}/f?pane={Uri.EscapeDataString(pane)}&path={Uri.EscapeDataString(path)}{(line is > 0 ? "#L" + line : "")}";

    [GeneratedRegex(@"(<[^>]*>)")]
    private static partial Regex Tag();

    /// <summary>
    /// Link file paths in rendered (already escaped) HTML: only in text nodes, never inside
    /// an existing link or a code block. Inline code chips are linked.
    /// </summary>
    public static string Linkify(string html, string machine, string pane)
    {
        var sb = new StringBuilder(html.Length + 64);
        int inA = 0, inPre = 0;
        foreach (var part in Tag().Split(html))
        {
            if (part.Length > 0 && part[0] == '<')
            {
                var t = part.ToLowerInvariant();
                if (t.StartsWith("<a ") || t == "<a>") inA++;
                else if (t == "</a>") inA = Math.Max(0, inA - 1);
                else if (t.StartsWith("<pre")) inPre++;
                else if (t == "</pre>") inPre = Math.Max(0, inPre - 1);
                sb.Append(part);
                continue;
            }
            if (inA > 0 || inPre > 0) { sb.Append(part); continue; }
            var text = WebUtility.HtmlDecode(part);
            var found = Find(text);
            if (found.Count == 0) { sb.Append(part); continue; }
            var pos = 0;
            foreach (var f in found)
            {
                sb.Append(WebUtility.HtmlEncode(text[pos..f.Index]));
                var shown = text.Substring(f.Index, f.Length);
                sb.Append("<a class=\"fpath\" href=\"").Append(WebUtility.HtmlEncode(Href(machine, pane, f.Path, f.Line)))
                  .Append("\" data-path=\"").Append(WebUtility.HtmlEncode(f.Path)).Append("\">")
                  .Append(WebUtility.HtmlEncode(shown)).Append("</a>");
                pos = f.Index + f.Length;
            }
            sb.Append(WebUtility.HtmlEncode(text[pos..]));
        }
        return sb.ToString();
    }

    // ---- what the hub refuses before asking a machine -----------------------------------

    /// <summary>Port of klark-link files::credential_like: secrets are never fetched.</summary>
    public static bool CredentialLike(string path)
    {
        var lp = "/" + path.Replace('\\', '/').ToLowerInvariant().TrimStart('/');
        foreach (var dir in new[] { "/.ssh/", "/.aws/", "/.gnupg/", "/.kube/", "/.docker/", "/.config/gcloud/", "/vault/", "/.password-store/" })
            if (lp.Contains(dir)) return true;
        var b = lp[(lp.LastIndexOf('/') + 1)..];
        return b == ".env" || b.EndsWith(".env") || b.StartsWith(".env.") || b.StartsWith("id_")
               || new[] { ".pem", ".key", ".p12", ".pfx" }.Any(b.EndsWith)
               || b is ".netrc" or ".npmrc" or ".pypirc"
               || new[] { "credential", "secret", "token" }.Any(b.Contains);
    }

    [GeneratedRegex(@"^/(?:Users|home)/[^/]+")]
    private static partial Regex HomeOf();

    /// <summary>
    /// Why the hub will not ask for this path, or null. Allowed: relative to the pane's cwd
    /// (no escaping with ..), "~/…", or absolute under the cwd or the cwd's home directory.
    /// </summary>
    public static string? Refusal(string path, string? cwd)
    {
        path = path.Trim();
        if (path.Length == 0) return "no path given";
        if (path.Length > 1024 || path.Contains('\0')) return "invalid path";
        if (CredentialLike(path)) return $"{path} looks like a credential; refusing";
        var segments = path.Split('/');
        if (segments.Contains("..") && !path.StartsWith('/')) return $"{path} leaves the pane's folder; refusing";
        if (path.StartsWith("~/")) return null;
        if (!path.StartsWith('/')) return null;
        if (segments.Contains("..")) return $"{path} is not a plain path; refusing";
        // macOS: /tmp, /var and /etc are links into /private; an agent may say either. The
        // machine resolves links before it hands anything over, so this only avoids a false refusal.
        var root = Unprivate((cwd ?? "").TrimEnd('/'));
        if (root.Length > 0 && (Unprivate(path) is var plain) && (plain == root || plain.StartsWith(root + "/"))) return null;
        var home = HomeOf().Match(root);
        if (home.Success && path.StartsWith(home.Value + "/")) return null;
        return $"{path} is outside the pane's folder and home; refusing";
    }

    private static string Unprivate(string p)
    {
        foreach (var d in new[] { "tmp", "var", "etc" })
        {
            var pre = "/private/" + d;
            if (p == pre || p.StartsWith(pre + "/")) return p[8..];
        }
        return p;
    }

    public static string Size(long b) => b >= 1 << 20 ? $"{b / 1048576.0:0.#} MB" : b >= 1024 ? $"{b / 1024.0:0} KB" : $"{b} B";
}

/// <summary>What the viewer draws for a file, decided from its name and first bytes.</summary>
public static class FileKinds
{
    public sealed record Kind(string View, string Lang, string Mime);

    private static readonly Dictionary<string, string> ByExt = new(StringComparer.OrdinalIgnoreCase)
    {
        ["rs"] = "rust", ["cs"] = "csharp", ["razor"] = "xml", ["go"] = "go", ["py"] = "python", ["js"] = "javascript", ["mjs"] = "javascript",
        ["cjs"] = "javascript", ["ts"] = "typescript", ["tsx"] = "typescript", ["jsx"] = "javascript", ["java"] = "java", ["kt"] = "kotlin",
        ["kts"] = "kotlin", ["swift"] = "swift", ["rb"] = "ruby", ["php"] = "php", ["c"] = "c", ["h"] = "c", ["cc"] = "cpp", ["cpp"] = "cpp",
        ["hpp"] = "cpp", ["m"] = "objectivec", ["mm"] = "objectivec", ["scala"] = "scala", ["clj"] = "clojure", ["ex"] = "elixir",
        ["exs"] = "elixir", ["erl"] = "erlang", ["hs"] = "haskell", ["lua"] = "lua", ["pl"] = "perl", ["r"] = "r", ["dart"] = "dart",
        ["zig"] = "zig", ["sh"] = "bash", ["bash"] = "bash", ["zsh"] = "bash", ["fish"] = "bash", ["ps1"] = "powershell", ["sql"] = "sql",
        ["json"] = "json", ["jsonc"] = "json", ["yaml"] = "yaml", ["yml"] = "yaml", ["toml"] = "ini", ["ini"] = "ini", ["cfg"] = "ini",
        ["conf"] = "ini", ["xml"] = "xml", ["html"] = "xml", ["htm"] = "xml", ["svg"] = "xml", ["css"] = "css", ["scss"] = "scss",
        ["less"] = "less", ["proto"] = "protobuf", ["graphql"] = "graphql", ["gradle"] = "gradle", ["tf"] = "hcl", ["hcl"] = "hcl",
        ["vue"] = "xml", ["svelte"] = "xml", ["csproj"] = "xml", ["fsproj"] = "xml", ["props"] = "xml", ["targets"] = "xml",
        ["md"] = "markdown", ["markdown"] = "markdown", ["mdx"] = "markdown", ["diff"] = "diff", ["patch"] = "diff", ["dockerfile"] = "dockerfile",
        ["mk"] = "makefile", ["cmake"] = "cmake", ["nix"] = "nix", ["vim"] = "vim", ["el"] = "lisp", ["fs"] = "fsharp", ["vb"] = "vbnet",
        ["ml"] = "ocaml", ["jl"] = "julia", ["groovy"] = "groovy", ["tex"] = "latex", ["bat"] = "dos", ["cmd"] = "dos",
    };

    private static readonly Dictionary<string, string> ByName = new(StringComparer.OrdinalIgnoreCase)
    {
        ["Dockerfile"] = "dockerfile", ["Containerfile"] = "dockerfile", ["Makefile"] = "makefile", ["GNUmakefile"] = "makefile",
        ["Justfile"] = "makefile", ["justfile"] = "makefile", [".gitignore"] = "bash", [".dockerignore"] = "bash", [".editorconfig"] = "ini",
        ["Gemfile"] = "ruby", ["Rakefile"] = "ruby", ["CMakeLists.txt"] = "cmake", ["Taskfile.yml"] = "yaml", ["go.mod"] = "go",
        ["Cargo.lock"] = "ini", [".bashrc"] = "bash", [".zshrc"] = "bash", ["Vagrantfile"] = "ruby", ["BUILD"] = "python",
    };

    public static Kind Detect(string name, ReadOnlySpan<byte> head)
    {
        var mime = AssetTypes.Sniff(head);
        var ext = name.Contains('.') ? name[(name.LastIndexOf('.') + 1)..].ToLowerInvariant() : "";
        if (AssetTypes.IsImage(mime)) return new("image", "", mime);
        if (mime == "application/pdf" || ext == "pdf") return new("pdf", "", "application/pdf");
        if (ext == "svg") return new("image", "xml", "image/svg+xml");
        if (LooksBinary(head)) return new("binary", "", "application/octet-stream");
        var lang = ByName.TryGetValue(name, out var n) ? n : name.StartsWith("Dockerfile", StringComparison.OrdinalIgnoreCase) ? "dockerfile"
            : ByExt.TryGetValue(ext, out var e) ? e : Shebang(head);
        var view = ext switch
        {
            "md" or "markdown" or "mdx" => "markdown",
            "csv" => "csv",
            "tsv" => "tsv",
            "json" or "jsonc" or "ipynb" => "json",
            "diff" or "patch" => "diff",
            _ => "code",
        };
        return new(view, lang, "text/plain; charset=utf-8");
    }

    private static string Shebang(ReadOnlySpan<byte> head)
    {
        if (head.Length < 3 || head[0] != '#' || head[1] != '!') return "";
        var nl = head.IndexOf((byte)'\n');
        var line = Encoding.UTF8.GetString(nl < 0 ? head : head[..nl]);
        foreach (var (k, v) in new[] { ("python", "python"), ("node", "javascript"), ("bash", "bash"), ("zsh", "bash"), ("/sh", "bash"), ("ruby", "ruby"), ("perl", "perl"), ("deno", "typescript"), ("bun", "typescript") })
            if (line.Contains(k)) return v;
        return "";
    }

    /// <summary>A NUL byte, or invalid UTF-8, in the first 8 KB means binary.</summary>
    public static bool LooksBinary(ReadOnlySpan<byte> head)
    {
        var s = head.Length > 8192 ? head[..8192] : head;
        if (s.IndexOf((byte)0) >= 0) return true;
        try
        {
            // The cut may land mid-character: trim up to 3 trailing bytes before validating.
            for (var trim = 0; trim < 4 && trim <= s.Length; trim++)
            {
                try { new UTF8Encoding(false, true).GetCharCount(s[..(s.Length - trim)]); return false; }
                catch (DecoderFallbackException) when (trim < 3 && head.Length > s.Length) { }
            }
        }
        catch (DecoderFallbackException) { }
        return true;
    }
}
