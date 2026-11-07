using System.Collections.Concurrent;
using System.Security.Claims;
using System.Text;
using Microsoft.AspNetCore.Authentication;
using Microsoft.AspNetCore.Authentication.Cookies;
using Microsoft.Extensions.Options;

namespace KlarkHub.Services;

/// <summary>
/// The file viewer (ADR 0011, G): a static page with its own script and stylesheet, plus a
/// small JSON + raw API. No Blazor circuit per opened file. A file is fetched from the
/// machine (get_file, protocol 4) only for a person who may see that machine; the hub
/// refuses credential-shaped and out-of-folder paths first, the machine checks again.
/// </summary>
public static class FileEndpoints
{
    private sealed record Cached(byte[] Bytes, string Name, string Path, DateTimeOffset At);

    private static readonly ConcurrentDictionary<string, Cached> Cache = new();
    private const long CacheBudget = 64L * 1024 * 1024;
    private static readonly TimeSpan CacheFor = TimeSpan.FromMinutes(2);

    public static void MapFileEndpoints(this WebApplication app)
    {
        app.MapGet("/m/{machine}/f", async (HttpContext ctx, IWebHostEnvironment env) =>
        {
            if (await UserAsync(ctx) is null)
                return Results.Redirect("/login?ReturnUrl=" + Uri.EscapeDataString(ctx.Request.Path + ctx.Request.QueryString));
            var file = env.WebRootFileProvider.GetFileInfo("viewer.html");
            ctx.Response.Headers.CacheControl = "no-cache";
            return Results.Stream(file.CreateReadStream(), "text/html; charset=utf-8");
        });

        app.MapGet("/api/files/{machine}", async (HttpContext ctx, string machine, string? pane, string? path,
            MachineRegistry registry, HubAccess access, IOptionsMonitor<HubOptions> options, ILoggerFactory logs) =>
        {
            var (status, file, error) = await FetchAsync(ctx, machine, pane, path, registry, access, options.CurrentValue.Files.MaxBytes, logs);
            if (file is null) return Results.Json(new { error }, statusCode: status);
            var kind = FileKinds.Detect(file.Name, file.Bytes.AsSpan(0, Math.Min(file.Bytes.Length, 8192)));
            var lines = kind.View is "image" or "pdf" or "binary" ? 0 : CountLines(file.Bytes);
            var raw = $"/api/files/{Uri.EscapeDataString(machine)}/raw?pane={Uri.EscapeDataString(pane!)}&path={Uri.EscapeDataString(path!)}";
            return Results.Json(new
            {
                name = file.Name, path = file.Path, size = file.Bytes.LongLength, sizeLabel = FileLinks.Size(file.Bytes.LongLength),
                view = kind.View, lang = kind.Lang, mime = kind.Mime, lines, raw, download = raw + "&download=1",
                machine, pane,
            });
        });

        app.MapGet("/api/files/{machine}/raw", async (HttpContext ctx, string machine, string? pane, string? path, bool? download,
            MachineRegistry registry, HubAccess access, IOptionsMonitor<HubOptions> options, ILoggerFactory logs) =>
        {
            var (status, file, error) = await FetchAsync(ctx, machine, pane, path, registry, access, options.CurrentValue.Files.MaxBytes, logs);
            if (file is null) return Results.Json(new { error }, statusCode: status);
            var kind = FileKinds.Detect(file.Name, file.Bytes.AsSpan(0, Math.Min(file.Bytes.Length, 8192)));
            var h = ctx.Response.Headers;
            h["X-Content-Type-Options"] = "nosniff";
            // Whatever the file holds, it can never run in the hub's origin.
            h["Content-Security-Policy"] = "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox";
            h.CacheControl = "private, max-age=60";
            var type = kind.View switch
            {
                "image" or "pdf" => kind.Mime,
                "binary" => "application/octet-stream",
                _ => "text/plain; charset=utf-8",
            };
            var name = AssetTypes.CleanName(file.Name);
            return download == true || kind.View == "binary"
                ? Results.File(file.Bytes, "application/octet-stream", name)
                : Results.File(file.Bytes, type);
        });
    }

    private static int CountLines(byte[] b)
    {
        if (b.Length == 0) return 0;
        var n = b.AsSpan().Count((byte)'\n');
        return b[^1] == '\n' ? n : n + 1;
    }

    private static async Task<ClaimsPrincipal?> UserAsync(HttpContext ctx)
    {
        var auth = await ctx.AuthenticateAsync(CookieAuthenticationDefaults.AuthenticationScheme);
        if (!auth.Succeeded && ApiAuth.SelectScheme(ctx) == ApiAuth.JwtScheme) auth = await ctx.AuthenticateAsync(ApiAuth.JwtScheme);
        return auth.Succeeded && auth.Principal is not null && ApiAuth.MachineOf(auth.Principal) is null ? auth.Principal : null;
    }

    private static async Task<(int Status, Cached? File, string? Error)> FetchAsync(HttpContext ctx, string machineId, string? pane, string? path,
        MachineRegistry registry, HubAccess access, long maxBytes, ILoggerFactory logs)
    {
        var user = await UserAsync(ctx);
        if (user is null) return (401, null, "sign in required");
        if (!access.CanSee(user, machineId)) return (403, null, $"machine {machineId} is not available to you");
        if (string.IsNullOrWhiteSpace(pane) || string.IsNullOrWhiteSpace(path)) return (400, null, "pane and path are required");
        var machine = registry.Get(machineId);
        if (machine is null) return (404, null, $"machine {machineId} is offline");
        var cwd = machine.Tree.Panes.FirstOrDefault(p => p.Id == pane)?.Cwd;
        if (FileLinks.Refusal(path, cwd) is { } refused) return (403, null, refused);

        var log = logs.CreateLogger("KlarkHub.Files");
        var key = $"{machineId}\n{pane}\n{path}";
        if (Cache.TryGetValue(key, out var hit) && DateTimeOffset.UtcNow - hit.At < CacheFor) return (200, hit, null);
        try
        {
            var r = await machine.GetFileAsync(pane, path, maxBytes, TimeSpan.FromSeconds(30));
            var data = Convert.FromBase64String(r?["data"]?.GetValue<string>() ?? "");
            if (data.LongLength > maxBytes)
                return (413, null, $"file is {FileLinks.Size(data.LongLength)}; limit {FileLinks.Size(maxBytes)}");
            var file = new Cached(data, r?["name"]?.GetValue<string>() ?? Path.GetFileName(path), r?["path"]?.GetValue<string>() ?? path, DateTimeOffset.UtcNow);
            log.LogInformation("file {Path} ({Size} bytes) fetched from {Machine}/{Pane} by {User}", file.Path, data.Length, machineId, pane, user.Identity?.Name);
            Remember(key, file);
            return (200, file, null);
        }
        catch (NotSupportedException e) { return (409, null, e.Message); }
        catch (TimeoutException) { return (504, null, $"{machineId} did not answer in time"); }
        catch (InvalidOperationException e)
        {
            log.LogInformation("file {Path} refused by {Machine} for {User}: {Error}", path, machineId, user.Identity?.Name, e.Message);
            return (e.Message.Contains("limit") ? 413 : 403, null, e.Message);
        }
        catch (FormatException) { return (502, null, "the machine sent a malformed file"); }
    }

    private static void Remember(string key, Cached file)
    {
        Cache[key] = file;
        var now = DateTimeOffset.UtcNow;
        foreach (var (k, v) in Cache) if (now - v.At > CacheFor) Cache.TryRemove(k, out _);
        while (Cache.Values.Sum(v => (long)v.Bytes.Length) > CacheBudget && Cache.Count > 1)
        {
            var oldest = Cache.MinBy(kv => kv.Value.At).Key;
            Cache.TryRemove(oldest, out _);
        }
    }
}
