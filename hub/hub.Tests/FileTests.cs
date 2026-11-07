using System.IO.Compression;
using System.Net;
using System.Net.Http.Headers;
using System.Net.Http.Json;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using KlarkHub.Services;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Mvc.Testing;
using Microsoft.Extensions.DependencyInjection;

namespace KlarkHub.Tests;

/// <summary>File links, the hub's own filter, type detection and sanitising (ADR 0011, G).</summary>
public class FileLinkTests
{
    [Theory]
    [InlineData("see src/main.rs for it", "src/main.rs", null)]
    [InlineData("at crates/klark-link/src/files.rs:42 now", "crates/klark-link/src/files.rs", 42)]
    [InlineData("open /Users/me/app/README.md", "/Users/me/app/README.md", null)]
    [InlineData("edit ~/notes/todo.txt please", "~/notes/todo.txt", null)]
    [InlineData("in ./docs/adr/0011.md:7:3", "./docs/adr/0011.md", 7)]
    [InlineData("Cargo.toml changed", "Cargo.toml", null)]
    [InlineData("the hub/Services/Assets dir", "hub/Services/Assets", null)]
    public void FindsPaths(string text, string path, int? line)
    {
        var f = Assert.Single(FileLinks.Find(text));
        Assert.Equal((path, line), (f.Path, f.Line));
    }

    [Theory]
    [InlineData("and/or w/o")]
    [InlineData("e.g. i.e. etc.")]
    [InlineData("visit https://example.com/a/b.md")]
    [InlineData("version 1.2.3 is out")]
    [InlineData("the /tmp dir")]
    public void IgnoresProse(string text) => Assert.Empty(FileLinks.Find(text));

    [Fact]
    public void Linkify_OnlyTextNodes_NeverInsideLinksOrPre()
    {
        var html = "<p>see <code>src/a.rs:3</code> and <a href=\"https://x.y/src/b.rs\">src/b.rs</a></p><pre><code>src/c.rs</code></pre>";
        var out_ = FileLinks.Linkify(html, "m", "p");
        Assert.Contains("<code><a class=\"fpath\" href=\"/m/m/f?pane=p&amp;path=src%2Fa.rs#L3\" data-path=\"src/a.rs\">src/a.rs:3</a></code>", out_);
        Assert.Contains("<a href=\"https://x.y/src/b.rs\">src/b.rs</a>", out_);
        Assert.Contains("<pre><code>src/c.rs</code></pre>", out_);
        // Escaped text stays escaped.
        Assert.StartsWith("&lt;script&gt; <a class=\"fpath\"", FileLinks.Linkify("&lt;script&gt; x/y/z.js", "m", "p"));
    }

    [Theory]
    [InlineData(".env")]
    [InlineData("app/.env.local")]
    [InlineData("certs/server.pem")]
    [InlineData("deploy.key")]
    [InlineData("/Users/me/.ssh/config")]
    [InlineData("id_ed25519")]
    [InlineData("gcp-credentials.json")]
    public void RefusesCredentials(string path)
    {
        Assert.True(FileLinks.CredentialLike(path));
        Assert.Contains("credential", FileLinks.Refusal(path, "/Users/me/app"));
    }

    [Theory]
    [InlineData("src/a.rs", null)]
    [InlineData("~/notes.md", null)]
    [InlineData("/Users/me/app/src/a.rs", null)]
    [InlineData("/Users/me/other/b.rs", null)] // the cwd's home
    [InlineData("../../etc/passwd", "leaves")]
    [InlineData("/etc/passwd", "outside")]
    [InlineData("/Users/other/x.rs", "outside")]
    [InlineData("/Users/me/app/../../other/x", "plain")]
    public void HubFilter(string path, string? refusal)
    {
        var r = FileLinks.Refusal(path, "/Users/me/app");
        if (refusal is null) Assert.Null(r); else Assert.Contains(refusal, r);
    }

    [Theory]
    [InlineData("main.rs", "code", "rust")]
    [InlineData("Dockerfile", "code", "dockerfile")]
    [InlineData("Makefile", "code", "makefile")]
    [InlineData("Justfile", "code", "makefile")]
    [InlineData(".gitignore", "code", "bash")]
    [InlineData("README.md", "markdown", "markdown")]
    [InlineData("data.csv", "csv", "")]
    [InlineData("data.tsv", "tsv", "")]
    [InlineData("package.json", "json", "json")]
    [InlineData("fix.patch", "diff", "diff")]
    [InlineData("logo.svg", "image", "xml")]
    public void DetectsKinds(string name, string view, string lang)
    {
        var k = FileKinds.Detect(name, "hello\n"u8);
        Assert.Equal((view, lang), (k.View, k.Lang));
    }

    [Fact]
    public void DetectsBySniffingAndShebang()
    {
        Assert.Equal("image", FileKinds.Detect("x.bin", new byte[] { 0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0 }).View);
        Assert.Equal("pdf", FileKinds.Detect("doc", "%PDF-1.7\n"u8).View);
        Assert.Equal("binary", FileKinds.Detect("a.out", new byte[] { 0x7F, 0x45, 0x4C, 0x46, 0, 1 }).View);
        Assert.Equal("binary", FileKinds.Detect("x.txt", new byte[] { 0xC3, 0x28, 0x41 }).View);
        Assert.Equal("python", FileKinds.Detect("run", "#!/usr/bin/env python3\nprint(1)\n"u8).Lang);
        Assert.Equal("bash", FileKinds.Detect("deploy", "#!/bin/sh\n"u8).Lang);
        Assert.Equal("code", FileKinds.Detect("x.txt", "héllo"u8).View);
    }

    [Fact]
    public void MarkdownPreview_IsSanitised()
    {
        var html = FileEndpoints.RenderMarkdown("# T\n<script>alert(1)</script>\n[x](javascript:alert(1)) [y](https://ok.example)\n![i](https://evil/i.png)\n```js\n<b>\n```");
        Assert.DoesNotContain("<script>", html);
        Assert.DoesNotContain("javascript:", html);
        Assert.DoesNotContain("<img", html);
        Assert.Contains("href=\"https://ok.example\"", html);
        Assert.Contains("<pre><code class=\"language-js\">&lt;b&gt;", html);
    }
}

/// <summary>The fetch flow end to end: a person asks, the hub asks a (fake) machine over the link, checks and serves.</summary>
public class FileFetchTests : IDisposable
{
    private const string Token = "box-token";
    private readonly TempDir _dir = new();
    private readonly WebApplicationFactory<Program> _factory;

    public FileFetchTests()
    {
        _factory = new WebApplicationFactory<Program>().WithWebHostBuilder(b =>
        {
            b.UseSetting("Hub:DataDir", _dir.Path);
            b.UseSetting("Hub:Auth:Mode", "");
            b.UseSetting("Hub:Auth:Ldap:Host", "");
            b.UseSetting("Hub:Machines:0:Name", "box");
            b.UseSetting("Hub:Machines:0:TokenSha256", TokenHash.Sha256Hex(Token));
            b.UseSetting("Hub:Files:MaxBytes", "1024");
            b.UseEnvironment("Production");
        });
    }

    public void Dispose()
    {
        _factory.Dispose();
        _dir.Dispose();
    }

    private HttpClient Client() => _factory.CreateClient(new WebApplicationFactoryClientOptions { AllowAutoRedirect = false });

    private async Task<HttpClient> Person()
    {
        var c = Client();
        var html = await c.GetStringAsync("/login");
        var token = Regex.Match(html, "name=\"__RequestVerificationToken\"[^>]*value=\"([^\"]+)\"").Groups[1].Value;
        var r = await c.PostAsync("/account/login", new FormUrlEncodedContent(new Dictionary<string, string>
        {
            ["username"] = "admin", ["password"] = HubAccounts.DefaultAdminPassword, ["returnUrl"] = "/", ["__RequestVerificationToken"] = token,
        }));
        Assert.Equal(HttpStatusCode.Redirect, r.StatusCode);
        return c;
    }

    private sealed record Box(HttpClient Http, string Session, StreamReader Events, HttpResponseMessage Stream) : IAsyncDisposable
    {
        private long _seq;

        public async Task<JsonNode> Next()
        {
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));
            while (true)
            {
                var node = JsonNode.Parse(await Events.ReadLineAsync(cts.Token) ?? throw new EndOfStreamException())!;
                if (node["seq"] is not null) return node;
            }
        }

        public Task Push(object msg)
        {
            var gz = new MemoryStream();
            using (var z = new GZipStream(gz, CompressionLevel.Fastest, leaveOpen: true)) z.Write(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(msg) + "\n"));
            var content = new ByteArrayContent(gz.ToArray());
            content.Headers.ContentType = new MediaTypeHeaderValue("application/x-ndjson");
            content.Headers.ContentEncoding.Add("gzip");
            return Http.PostAsync($"/agent/push?session={Session}&seq={Interlocked.Increment(ref _seq)}", content);
        }

        public ValueTask DisposeAsync() { Stream.Dispose(); return ValueTask.CompletedTask; }
    }

    private async Task<Box> Machine(int protocol = 5)
    {
        var c = Client();
        c.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", Token);
        var r = await c.PostAsJsonAsync("/agent/session", new { type = "hello", protocol, machine_id = "box", machine_name = "box", klark_version = "test" });
        var session = JsonDocument.Parse(await r.Content.ReadAsStringAsync()).RootElement.GetProperty("session_id").GetString()!;
        var s = await c.SendAsync(new HttpRequestMessage(HttpMethod.Get, $"/agent/events?session={session}&after=0"), HttpCompletionOption.ResponseHeadersRead);
        var reg = _factory.Services.GetRequiredService<MachineRegistry>();
        for (var i = 0; i < 100 && reg.Get("box") is null; i++) await Task.Delay(20);
        return new Box(c, session, new StreamReader(await s.Content.ReadAsStreamAsync()), s);
    }

    /// <summary>Answer the next get_file the hub sends, after checking what it asked for.</summary>
    private static async Task Answer(Box box, Func<JsonNode, object> reply)
    {
        while (true)
        {
            var cmd = await box.Next();
            if (cmd["type"]?.GetValue<string>() != "get_file") continue;
            await box.Push(reply(cmd));
            return;
        }
    }

    [Fact]
    public async Task SignedOut_IsRefused_AndTheViewerPageRedirects()
    {
        Assert.Equal(HttpStatusCode.Unauthorized, (await Client().GetAsync("/api/files/box?pane=p1&path=a.rs")).StatusCode);
        var page = await Client().GetAsync("/m/box/f?pane=p1&path=a.rs");
        Assert.Equal(HttpStatusCode.Redirect, page.StatusCode);
        Assert.StartsWith("/login", page.Headers.Location!.ToString());
    }

    [Fact]
    public async Task Fetch_GoesToTheMachine_AndServesMetaRawAndViewer()
    {
        await using var box = await Machine();
        var person = await Person();
        var ask = person.GetAsync("/api/files/box?pane=p1&path=src%2Fa.rs%3A3");
        await Answer(box, cmd =>
        {
            Assert.Equal(("p1", "src/a.rs:3", 1024L), (cmd["pane_id"]!.GetValue<string>(), cmd["path"]!.GetValue<string>(), cmd["max_bytes"]!.GetValue<long>()));
            return new { type = "result", id = cmd["id"]!.GetValue<string>(), result = new { name = "a.rs", path = "/w/src/a.rs", size = 12, data = Convert.ToBase64String("fn main() {}"u8.ToArray()) } };
        });
        var meta = JsonNode.Parse(await (await ask).Content.ReadAsStringAsync())!;
        Assert.Equal(("a.rs", "code", "rust", 1), (meta["name"]!.GetValue<string>(), meta["view"]!.GetValue<string>(), meta["lang"]!.GetValue<string>(), meta["lines"]!.GetValue<int>()));
        // Raw is served from the hub's short cache: no second trip to the machine.
        var raw = await person.GetAsync(meta["raw"]!.GetValue<string>());
        Assert.Equal("fn main() {}", await raw.Content.ReadAsStringAsync());
        Assert.Equal("nosniff", raw.Headers.GetValues("X-Content-Type-Options").Single());
        Assert.Contains("sandbox", raw.Headers.GetValues("Content-Security-Policy").Single());
        Assert.Equal("text/plain", raw.Content.Headers.ContentType!.MediaType);
        var dl = await person.GetAsync(meta["download"]!.GetValue<string>());
        Assert.Equal("attachment", dl.Content.Headers.ContentDisposition!.DispositionType);
        var page = await person.GetAsync("/m/box/f?pane=p1&path=src/a.rs");
        Assert.Contains("viewer.js", await page.Content.ReadAsStringAsync());
    }

    [Fact]
    public async Task Credentials_AreRefusedByTheHub_BeforeTheMachineIsAsked()
    {
        await using var box = await Machine();
        var person = await Person();
        var r = await person.GetAsync("/api/files/box?pane=p1&path=.env");
        Assert.Equal(HttpStatusCode.Forbidden, r.StatusCode);
        Assert.Contains("credential", await r.Content.ReadAsStringAsync());
    }

    [Fact]
    public async Task MachineRefusal_AndOverLimit_ReachThePerson()
    {
        await using var box = await Machine();
        var person = await Person();
        var ask = person.GetAsync("/api/files/box?pane=p1&path=big.bin");
        await Answer(box, cmd => new { type = "result", id = cmd["id"]!.GetValue<string>(), error = "file is 12 MB; limit 10 MB" });
        var r = await ask;
        Assert.Equal(HttpStatusCode.RequestEntityTooLarge, r.StatusCode);
        Assert.Contains("file is 12 MB; limit 10 MB", await r.Content.ReadAsStringAsync());

        // A machine that sends more than Hub:Files:MaxBytes anyway is refused by the hub too.
        ask = person.GetAsync("/api/files/box?pane=p1&path=x.txt");
        await Answer(box, cmd => new { type = "result", id = cmd["id"]!.GetValue<string>(), result = new { name = "x.txt", path = "/w/x.txt", size = 1025, data = Convert.ToBase64String(new byte[1025]) } });
        r = await ask;
        Assert.Equal(HttpStatusCode.RequestEntityTooLarge, r.StatusCode);
        Assert.Contains("limit 1 KB", await r.Content.ReadAsStringAsync());
    }

    [Fact]
    public async Task OldMachines_AreToldToUpdate()
    {
        await using var box = await Machine(protocol: 4);
        var person = await Person();
        var r = await person.GetAsync("/api/files/box?pane=p1&path=a.rs");
        Assert.Equal(HttpStatusCode.Conflict, r.StatusCode);
        Assert.Contains("update klark", await r.Content.ReadAsStringAsync());
    }

    [Fact]
    public void DefaultLimitIsTenMegabytes() => Assert.Equal(10 * 1024 * 1024, new HubOptions().Files.MaxBytes);
}
