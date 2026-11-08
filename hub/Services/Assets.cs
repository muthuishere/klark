using System.Security.Claims;
using System.Security.Cryptography;
using System.Text;
using Microsoft.Data.Sqlite;
using Microsoft.Extensions.Options;

namespace KlarkHub.Services;

/// <summary>One recorded asset (ADR 0011, B). Direction is "to_agent" or "from_agent".</summary>
public sealed record Asset(long Id, string Sha256, long Size, string Mime, string Name, DateTimeOffset Ts,
    string Machine, string PaneId, string Direction, string By)
{
    public bool IsImage => AssetTypes.IsImage(Mime);
    public string Url => "/assets/" + Sha256;
}

public sealed class AssetTooLargeException(long max) : Exception($"larger than {max} bytes")
{
    public long Max { get; } = max;
}

/// <summary>What the store wrote: the content hash, the size, and the first bytes for sniffing.</summary>
public sealed record StoredObject(string Sha256, long Size, byte[] Head, bool Existed);

/// <summary>Content-addressed bytes. Keys are lowercase hex SHA-256.</summary>
public interface IAssetStore
{
    /// <summary>Stream <paramref name="source"/> in, refusing more than <paramref name="maxBytes"/>. An identical object is kept once.</summary>
    Task<StoredObject> PutAsync(Stream source, long maxBytes, CancellationToken ct);
    Stream? OpenRead(string sha256);
    bool Exists(string sha256);
    void Delete(string sha256);
    IEnumerable<string> List();
}

/// <summary>
/// The hub's own store (ADR 0011 as amended): files under
/// &lt;DataDir&gt;/assets/sha256/&lt;aa&gt;/&lt;hex&gt;, written to a temp file, fsynced,
/// then renamed into place. Directories are 0700, files 0600.
/// </summary>
public sealed class FileAssetStore : IAssetStore
{
    private const UnixFileMode DirMode = UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute;
    private const UnixFileMode FileMode600 = UnixFileMode.UserRead | UnixFileMode.UserWrite;
    private readonly string _root;
    private readonly string _tmp;

    public FileAssetStore(IOptions<HubOptions> options)
    {
        _root = Path.Combine(options.Value.DataDir, "assets", "sha256");
        _tmp = Path.Combine(options.Value.DataDir, "assets", "tmp");
        MakeDir(Path.Combine(options.Value.DataDir, "assets"));
        MakeDir(_root);
        MakeDir(_tmp);
        // A crash mid-write leaves a temp file; it never became an object.
        foreach (var f in Directory.EnumerateFiles(_tmp)) TryDelete(f);
    }

    public static bool IsSha(string s) => s.Length == 64 && s.All(c => c is >= '0' and <= '9' or >= 'a' and <= 'f');

    private string PathOf(string sha) => Path.Combine(_root, sha[..2], sha);

    public async Task<StoredObject> PutAsync(Stream source, long maxBytes, CancellationToken ct)
    {
        var tmp = Path.Combine(_tmp, Guid.NewGuid().ToString("N"));
        var head = new byte[512];
        var headLen = 0;
        long size = 0;
        try
        {
            using var sha = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
            var opts = new FileStreamOptions { Mode = FileMode.CreateNew, Access = FileAccess.Write };
            if (!OperatingSystem.IsWindows()) opts.UnixCreateMode = FileMode600;
            await using (var file = new FileStream(tmp, opts))
            {
                var buf = new byte[81920];
                int n;
                while ((n = await source.ReadAsync(buf, ct)) > 0)
                {
                    size += n;
                    if (size > maxBytes) throw new AssetTooLargeException(maxBytes);
                    if (headLen < head.Length)
                    {
                        var take = Math.Min(n, head.Length - headLen);
                        Array.Copy(buf, 0, head, headLen, take);
                        headLen += take;
                    }
                    sha.AppendData(buf, 0, n);
                    await file.WriteAsync(buf.AsMemory(0, n), ct);
                }
                file.Flush(flushToDisk: true);
            }
            var hex = Convert.ToHexString(sha.GetHashAndReset()).ToLowerInvariant();
            var final = PathOf(hex);
            var existed = File.Exists(final);
            if (existed)
            {
                TryDelete(tmp);
            }
            else
            {
                MakeDir(Path.GetDirectoryName(final)!);
                File.Move(tmp, final, overwrite: true);
            }
            return new StoredObject(hex, size, head[..headLen], existed);
        }
        catch
        {
            TryDelete(tmp);
            throw;
        }
    }

    public Stream? OpenRead(string sha256)
    {
        if (!IsSha(sha256)) return null;
        try { return new FileStream(PathOf(sha256), FileMode.Open, FileAccess.Read, FileShare.Read | FileShare.Delete, 81920, useAsync: true); }
        catch (FileNotFoundException) { return null; }
        catch (DirectoryNotFoundException) { return null; }
    }

    public bool Exists(string sha256) => IsSha(sha256) && File.Exists(PathOf(sha256));

    public void Delete(string sha256)
    {
        if (IsSha(sha256)) TryDelete(PathOf(sha256));
    }

    public IEnumerable<string> List() =>
        Directory.Exists(_root)
            ? Directory.EnumerateFiles(_root, "*", SearchOption.AllDirectories).Select(Path.GetFileName).OfType<string>().Where(IsSha)
            : Enumerable.Empty<string>();

    private static void MakeDir(string path)
    {
        if (OperatingSystem.IsWindows()) Directory.CreateDirectory(path);
        else Directory.CreateDirectory(path, DirMode);
    }

    private static void TryDelete(string path)
    {
        try { File.Delete(path); } catch (IOException) { } catch (UnauthorizedAccessException) { }
    }
}

/// <summary>Types from magic bytes only (ADR 0011, B); never the name or a claimed type.</summary>
public static class AssetTypes
{
    public static string Sniff(ReadOnlySpan<byte> b)
    {
        // Bytes, not a u8 string: "\x89"u8 would be UTF-8 for U+0089, two bytes.
        if (Has(b, [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])) return "image/png";
        if (Has(b, [0xFF, 0xD8, 0xFF])) return "image/jpeg";
        if (Has(b, "GIF87a"u8) || Has(b, "GIF89a"u8)) return "image/gif";
        if (b.Length >= 12 && Has(b, "RIFF"u8) && b[8..12].SequenceEqual("WEBP"u8)) return "image/webp";
        if (Ftyp(b, "heic", "heix", "hevc", "mif1", "heim", "msf1")) return "image/heic";
        if (Ftyp(b, "avif", "avis")) return "image/avif";
        if (Has(b, "%PDF-"u8)) return "application/pdf";
        if (Has(b, "PK\x03\x04"u8) || Has(b, "PK\x05\x06"u8)) return "application/zip";
        if (b.IndexOf((byte)0) >= 0) return "application/octet-stream";
        if (!LooksUtf8(b)) return "application/octet-stream";
        // Markup is recognised so it can be refused as markup when served.
        var text = Encoding.UTF8.GetString(b).TrimStart('﻿', ' ', '\t', '\r', '\n').ToLowerInvariant();
        if (text.StartsWith("<svg") || (text.StartsWith("<?xml") && text.Contains("<svg"))) return "image/svg+xml";
        if (text.StartsWith("<!doctype html") || text.StartsWith("<html") || text.StartsWith("<head") || text.StartsWith("<script")) return "text/html";
        if (text.StartsWith("<?xml")) return "application/xml";
        return "text/plain";
    }

    /// <summary>Images that browsers draw and that carry no script. SVG is not one of them.</summary>
    public static bool IsImage(string mime) => mime is "image/png" or "image/jpeg" or "image/gif" or "image/webp" or "image/heic" or "image/avif";

    /// <summary>The Content-Type to send: markup never goes out as markup.</summary>
    public static string ServeAs(string mime) =>
        mime is "text/html" or "image/svg+xml" || mime.EndsWith("/xml") || mime.EndsWith("+xml")
            ? "application/octet-stream"
            : mime == "text/plain" ? "text/plain; charset=utf-8" : mime;

    public static string Extension(string mime) => mime switch
    {
        "image/png" => "png", "image/jpeg" => "jpg", "image/gif" => "gif", "image/webp" => "webp",
        "image/heic" => "heic", "image/avif" => "avif", "application/pdf" => "pdf", "application/zip" => "zip",
        "text/plain" => "txt", _ => "bin",
    };

    private static bool Has(ReadOnlySpan<byte> b, ReadOnlySpan<byte> sig) => b.StartsWith(sig);

    private static bool Ftyp(ReadOnlySpan<byte> b, params string[] brands)
    {
        if (b.Length < 12 || !b[4..8].SequenceEqual("ftyp"u8)) return false;
        var brand = Encoding.ASCII.GetString(b[8..12]);
        return brands.Contains(brand);
    }

    /// <summary>Valid UTF-8, allowing a sequence cut off at the end of the sniffed head.</summary>
    private static bool LooksUtf8(ReadOnlySpan<byte> b)
    {
        for (var cut = 0; cut < 4 && cut <= b.Length; cut++)
        {
            if (System.Text.Unicode.Utf8.IsValid(b[..(b.Length - cut)])) return true;
        }
        return false;
    }

    /// <summary>A file name fit for a header and a list: no path, no control characters, at most 120 chars.</summary>
    public static string CleanName(string? name)
    {
        var n = Path.GetFileName((name ?? "").Replace('\\', '/'));
        n = new string(n.Where(c => !char.IsControl(c) && c is not '"' and not '/').ToArray()).Trim();
        if (n.Length > 120) n = n[^120..];
        return n.Length == 0 ? "file" : n;
    }
}

/// <summary>The result of sending an image to an agent.</summary>
public sealed record SendAssetOutcome(int Status, Asset? Asset, string? Path, string? Error);

/// <summary>
/// Assets end to end (ADR 0011): the hub.db `assets` table, sending images to
/// agents with fetch_asset, offers from agents, and the retention sweep.
/// Every asset is temporary: nothing is kept past RetentionHours.
/// </summary>
public sealed class AssetService : BackgroundService
{
    private readonly IAssetStore _store;
    private readonly IOptionsMonitor<HubOptions> _options;
    private readonly MachineRegistry _registry;
    private readonly HubAccess _access;
    private readonly ILogger<AssetService> _log;
    private readonly object _schemaLock = new();
    private bool _schemaReady;

    public AssetService(IAssetStore store, IOptionsMonitor<HubOptions> options, MachineRegistry registry, HubAccess access,
        ILogger<AssetService> log)
    {
        _store = store;
        _options = options;
        _registry = registry;
        _access = access;
        _log = log;
    }

    public AssetOptions Options => _options.CurrentValue.Assets;

    /// <summary>Raised with (machine, pane) when an agent offers a new file.</summary>
    public event Action<string, string>? Offered;

    // ---- the table -----------------------------------------------------------

    private SqliteConnection Open()
    {
        var dir = _options.CurrentValue.DataDir;
        Directory.CreateDirectory(dir);
        var db = new SqliteConnection(new SqliteConnectionStringBuilder
        {
            DataSource = Path.Combine(dir, "hub.db"), Pooling = false, DefaultTimeout = 30,
        }.ToString());
        db.Open();
        lock (_schemaLock)
        {
            if (!_schemaReady)
            {
                using var cmd = db.CreateCommand();
                cmd.CommandText = """
                    PRAGMA journal_mode = WAL;
                    CREATE TABLE IF NOT EXISTS assets (
                      id INTEGER PRIMARY KEY AUTOINCREMENT, sha256 TEXT NOT NULL, size INTEGER NOT NULL, mime TEXT NOT NULL,
                      name TEXT NOT NULL, ts INTEGER NOT NULL, machine TEXT NOT NULL, pane_id TEXT NOT NULL,
                      direction TEXT NOT NULL CHECK (direction IN ('to_agent', 'from_agent')), by TEXT NOT NULL);
                    CREATE INDEX IF NOT EXISTS assets_sha ON assets (sha256);
                    CREATE INDEX IF NOT EXISTS assets_pane ON assets (machine, pane_id, direction, ts);
                    CREATE INDEX IF NOT EXISTS assets_ts ON assets (ts);
                    """;
                cmd.ExecuteNonQuery();
                _schemaReady = true;
            }
        }
        return db;
    }

    private Asset Record(StoredObject obj, string mime, string name, string machine, string pane, string direction, string by)
    {
        var ts = DateTimeOffset.UtcNow;
        using var db = Open();
        using var cmd = db.CreateCommand();
        cmd.CommandText = """
            INSERT INTO assets (sha256, size, mime, name, ts, machine, pane_id, direction, by)
            VALUES ($sha, $size, $mime, $name, $ts, $machine, $pane, $dir, $by) RETURNING id
            """;
        cmd.Parameters.AddWithValue("$sha", obj.Sha256);
        cmd.Parameters.AddWithValue("$size", obj.Size);
        cmd.Parameters.AddWithValue("$mime", mime);
        cmd.Parameters.AddWithValue("$name", name);
        cmd.Parameters.AddWithValue("$ts", ts.ToUnixTimeMilliseconds());
        cmd.Parameters.AddWithValue("$machine", machine);
        cmd.Parameters.AddWithValue("$pane", pane);
        cmd.Parameters.AddWithValue("$dir", direction);
        cmd.Parameters.AddWithValue("$by", by);
        var id = Convert.ToInt64(cmd.ExecuteScalar());
        return new Asset(id, obj.Sha256, obj.Size, mime, name, ts, machine, pane, direction, by);
    }

    private List<Asset> Query(string where, params (string, object)[] args)
    {
        using var db = Open();
        using var cmd = db.CreateCommand();
        cmd.CommandText = "SELECT id, sha256, size, mime, name, ts, machine, pane_id, direction, by FROM assets WHERE " + where;
        foreach (var (k, v) in args) cmd.Parameters.AddWithValue(k, v);
        var list = new List<Asset>();
        using var r = cmd.ExecuteReader();
        while (r.Read())
        {
            list.Add(new Asset(r.GetInt64(0), r.GetString(1), r.GetInt64(2), r.GetString(3), r.GetString(4),
                DateTimeOffset.FromUnixTimeMilliseconds(r.GetInt64(5)), r.GetString(6), r.GetString(7), r.GetString(8), r.GetString(9)));
        }
        return list;
    }

    private long Cutoff => DateTimeOffset.UtcNow.AddHours(-Options.RetentionHours).ToUnixTimeMilliseconds();

    /// <summary>The newest live record of a hash that <paramref name="visible"/> accepts.</summary>
    public Asset? Find(string sha256, Func<Asset, bool> visible) =>
        FileAssetStore.IsSha(sha256)
            ? Query("sha256 = $sha AND ts >= $cut ORDER BY ts DESC", ("$sha", sha256), ("$cut", Cutoff)).FirstOrDefault(visible)
            : null;

    public List<Asset> Offers(string machine, string pane) =>
        Query("machine = $m AND pane_id = $p AND direction = 'from_agent' AND ts >= $cut ORDER BY ts DESC LIMIT 50",
            ("$m", machine), ("$p", pane), ("$cut", Cutoff));

    public Stream? OpenRead(string sha256) => _store.OpenRead(sha256);

    // ---- person to agent -----------------------------------------------------

    /// <summary>
    /// A person sends an image to a pane's agent: check, store, record, then
    /// fetch_asset to the machine and wait for the path it wrote.
    /// </summary>
    public async Task<SendAssetOutcome> SendToAgentAsync(ClaimsPrincipal user, string machineId, string paneId, Stream body,
        string? name, string? caption, CancellationToken ct, bool prompt = true)
    {
        // Checked here, on the server, whatever the page showed (STATUS S-06).
        if (!_access.CanType(user, machineId)) return new(403, null, null, $"you may not type into {machineId}");
        var machine = _registry.Get(machineId);
        if (machine is null) return new(404, null, null, $"machine {machineId} is not linked to this hub");
        if (machine.Protocol < 3) return new(409, null, null, $"machine {machineId} links with protocol {machine.Protocol}; update klark there to receive images");

        StoredObject obj;
        try { obj = await _store.PutAsync(body, Options.MaxImageBytes, ct); }
        catch (AssetTooLargeException e) { return new(413, null, null, $"image is {e.Message}"); }
        var mime = AssetTypes.Sniff(obj.Head);
        if (!AssetTypes.IsImage(mime))
        {
            if (!obj.Existed && Query("sha256 = $sha", ("$sha", obj.Sha256)).Count == 0) _store.Delete(obj.Sha256);
            return new(415, null, null, $"not an image (looks like {mime}); send png, jpeg, gif, webp, heic or avif");
        }
        var clean = AssetTypes.CleanName(name);
        if (!clean.Contains('.')) clean += "." + AssetTypes.Extension(mime);
        var asset = Record(obj, mime, clean, machineId, paneId, "to_agent", "user:" + user.Identity?.Name);
        try
        {
            var path = await machine.FetchAssetAsync(asset, caption ?? "", TimeSpan.FromSeconds(60), prompt);
            _log.LogInformation("asset {Sha} ({Size} bytes) delivered to {Machine}/{Pane}", asset.Sha256[..12], asset.Size, machineId, paneId);
            return new(200, asset, path, null);
        }
        catch (NotSupportedException e) { return new(409, asset, null, e.Message); }
        catch (TimeoutException) { return new(504, asset, null, $"{machineId} did not fetch the image in time"); }
        catch (InvalidOperationException e) { return new(502, asset, null, e.Message); }
    }

    // ---- agent to person -----------------------------------------------------

    /// <summary>A machine offers a file for one of its panes (`klark offer`).</summary>
    public async Task<(int Status, Asset? Asset, string? Error)> OfferAsync(string machineId, string paneId, string? name,
        Stream body, CancellationToken ct)
    {
        StoredObject obj;
        try { obj = await _store.PutAsync(body, Options.MaxFileBytes, ct); }
        catch (AssetTooLargeException e) { return (413, null, $"file is {e.Message}"); }
        var mime = AssetTypes.Sniff(obj.Head);
        var asset = Record(obj, mime, AssetTypes.CleanName(name), machineId, paneId, "from_agent", "machine:" + machineId);
        _log.LogInformation("asset {Sha} ({Size} bytes, {Mime}) offered by {Machine}/{Pane}{Dedup}", obj.Sha256[..12], obj.Size, mime,
            machineId, paneId, obj.Existed ? " (already stored)" : "");
        try { Offered?.Invoke(machineId, paneId); }
        catch (Exception e) { _log.LogWarning(e, "offer handler failed"); }
        return (200, asset, null);
    }

    // ---- retention -----------------------------------------------------------

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        using var timer = new PeriodicTimer(TimeSpan.FromMinutes(10));
        do
        {
            try { Sweep(); }
            catch (Exception e) { _log.LogWarning(e, "asset sweep failed"); }
        } while (await timer.WaitForNextTickAsync(stoppingToken).ConfigureAwait(false) is true);
    }

    /// <summary>Delete rows past retention, then every object no live row names.</summary>
    public void Sweep()
    {
        int rows;
        HashSet<string> live;
        using (var db = Open())
        {
            using (var del = db.CreateCommand())
            {
                del.CommandText = "DELETE FROM assets WHERE ts < $cut";
                del.Parameters.AddWithValue("$cut", Cutoff);
                rows = del.ExecuteNonQuery();
            }
            using var sel = db.CreateCommand();
            sel.CommandText = "SELECT DISTINCT sha256 FROM assets";
            live = new HashSet<string>();
            using var r = sel.ExecuteReader();
            while (r.Read()) live.Add(r.GetString(0));
        }
        var files = 0;
        foreach (var sha in _store.List().ToList())
        {
            if (live.Contains(sha)) continue;
            _store.Delete(sha);
            files++;
        }
        if (rows > 0 || files > 0) _log.LogInformation("asset sweep: {Rows} records and {Files} files past {Hours} h removed", rows, files, Options.RetentionHours);
    }
}
