using System.IO.Compression;
using System.Net;
using System.Net.Http.Headers;
using System.Net.Http.Json;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using KlarkHub.Services;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Mvc.Testing;
using Microsoft.Extensions.DependencyInjection;

namespace KlarkHub.Tests;

/// <summary>The machine link over plain HTTP (ADR 0013): session, NDJSON events down, POST segments up.</summary>
public class AgentLinkTests : IDisposable
{
    private const string Token = "box-token";
    private readonly TempDir _dir = new();
    private readonly WebApplicationFactory<Program> _factory;

    public AgentLinkTests()
    {
        _factory = new WebApplicationFactory<Program>().WithWebHostBuilder(b =>
        {
            b.UseSetting("Hub:DataDir", _dir.Path);
            b.UseSetting("Hub:Auth:Mode", "");
            b.UseSetting("Hub:Auth:Ldap:Host", "");
            b.UseSetting("Hub:Machines:0:Name", "box");
            b.UseSetting("Hub:Machines:0:TokenSha256", TokenHash.Sha256Hex(Token));
            b.UseEnvironment("Production");
        });
        Registry.Heartbeat = TimeSpan.FromMilliseconds(300);
    }

    public void Dispose()
    {
        _factory.Dispose();
        _dir.Dispose();
    }

    private MachineRegistry Registry => _factory.Services.GetRequiredService<MachineRegistry>();

    private HttpClient Machine(string token = Token)
    {
        var c = _factory.CreateClient(new WebApplicationFactoryClientOptions { AllowAutoRedirect = false });
        c.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", token);
        return c;
    }

    private static object Hello(int protocol = 4) =>
        new { type = "hello", protocol, machine_id = "box", machine_name = "box-host", klark_version = "test" };

    private static async Task<string> OpenSession(HttpClient c)
    {
        var r = await c.PostAsJsonAsync("/agent/session", Hello());
        Assert.Equal(HttpStatusCode.OK, r.StatusCode);
        var body = JsonDocument.Parse(await r.Content.ReadAsStringAsync()).RootElement;
        return body.GetProperty("session_id").GetString()!;
    }

    private static Task<HttpResponseMessage> Push(HttpClient c, string session, long seq, params object[] msgs)
    {
        var lines = string.Concat(msgs.Select(m => JsonSerializer.Serialize(m) + "\n"));
        var gz = new MemoryStream();
        using (var z = new GZipStream(gz, CompressionLevel.Fastest, leaveOpen: true)) z.Write(Encoding.UTF8.GetBytes(lines));
        var content = new ByteArrayContent(gz.ToArray());
        content.Headers.ContentType = new MediaTypeHeaderValue("application/x-ndjson");
        content.Headers.ContentEncoding.Add("gzip");
        return c.PostAsync($"/agent/push?session={session}&seq={seq}", content);
    }

    /// <summary>The event stream, read line by line.</summary>
    private sealed class Events : IAsyncDisposable
    {
        public HttpResponseMessage Response { get; private init; } = null!;
        private StreamReader _reader = null!;

        public static async Task<Events> Open(HttpClient c, string session, long after = 0)
        {
            var r = await c.SendAsync(new HttpRequestMessage(HttpMethod.Get, $"/agent/events?session={session}&after={after}"),
                HttpCompletionOption.ResponseHeadersRead);
            return new Events { Response = r, _reader = new StreamReader(await r.Content.ReadAsStreamAsync()) };
        }

        /// <summary>The next line that is not a heartbeat.</summary>
        public async Task<JsonNode> Next(TimeSpan? timeout = null)
        {
            using var cts = new CancellationTokenSource(timeout ?? TimeSpan.FromSeconds(10));
            while (true)
            {
                var line = await _reader.ReadLineAsync(cts.Token) ?? throw new EndOfStreamException();
                var node = JsonNode.Parse(line)!;
                if (node["seq"] is not null) return node;
            }
        }

        public async Task<string?> RawLine(TimeSpan timeout)
        {
            using var cts = new CancellationTokenSource(timeout);
            return await _reader.ReadLineAsync(cts.Token);
        }

        public async ValueTask DisposeAsync()
        {
            Response.Dispose();
            await Task.CompletedTask;
        }
    }

    private async Task<MachineConnection> Linked()
    {
        for (var i = 0; i < 100; i++)
        {
            if (Registry.Get("box") is { } m) return m;
            await Task.Delay(20);
        }
        throw new TimeoutException("box never linked");
    }

    [Fact]
    public async Task Session_RequiresAMachineToken_ConfigOrDb()
    {
        Assert.Equal(HttpStatusCode.Unauthorized, (await Machine("wrong").PostAsJsonAsync("/agent/session", Hello())).StatusCode);
        Assert.Equal(HttpStatusCode.Unauthorized, (await Machine("wrong").GetAsync("/agent")).StatusCode);
        Assert.Equal(HttpStatusCode.OK, (await Machine().GetAsync("/agent")).StatusCode);
        Assert.Equal(HttpStatusCode.BadRequest, (await Machine().PostAsJsonAsync("/agent/session", Hello(protocol: 3))).StatusCode);

        var session = await OpenSession(Machine());
        Assert.Equal(32, session.Length);
        var m = await Linked();
        Assert.Equal("box-host", m.Host);
        Assert.Equal(4, m.Protocol);

        // A machine created in hub.db (the device flow) links the same way.
        var (dbToken, error) = _factory.Services.GetRequiredService<HubAccounts>().CreateMachine("laptop", "admin");
        Assert.Null(error);
        await OpenSession(Machine(dbToken!));
        Assert.NotNull(Registry.Get("laptop"));

        // Another machine's token cannot use this session.
        Assert.Equal(HttpStatusCode.Unauthorized, (await Push(Machine(dbToken!), session, 1, new { type = "pong" })).StatusCode);
        Assert.Equal(HttpStatusCode.NotFound, (await Push(Machine(), "nope", 1, new { type = "pong" })).StatusCode);
    }

    [Fact]
    public async Task Events_DeliverCommandsAtOnce_WithSeq_AndHeartbeatWhenIdle()
    {
        var c = Machine();
        var session = await OpenSession(c);
        await using var ev = await Events.Open(c, session);
        Assert.Equal("application/x-ndjson", ev.Response.Content.Headers.ContentType?.MediaType);
        Assert.Equal("no", ev.Response.Headers.GetValues("X-Accel-Buffering").Single());

        // Idle: only heartbeats.
        Assert.Equal("{\"type\":\"ping\"}", await ev.RawLine(TimeSpan.FromSeconds(5)));

        var m = await Linked();
        await m.SendInputAsync("p1", "ls");
        var first = await ev.Next(TimeSpan.FromSeconds(2));
        Assert.Equal("input", first["type"]!.GetValue<string>());
        Assert.Equal("ls", first["text"]!.GetValue<string>());
        Assert.Equal(1, first["seq"]!.GetValue<long>());
        await m.SendKeysAsync("p1", "enter");
        Assert.Equal(2, (await ev.Next())["seq"]!.GetValue<long>());
    }

    [Fact]
    public async Task Events_ResumeAfterADrop_FromLastSeq()
    {
        var c = Machine();
        var session = await OpenSession(c);
        var m = await Linked();
        await m.SendInputAsync("p1", "a");
        await m.SendInputAsync("p1", "b");
        await m.SendInputAsync("p1", "c");
        await using var ev = await Events.Open(c, session, after: 1);
        Assert.Equal("b", (await ev.Next())["text"]!.GetValue<string>());
        Assert.Equal("c", (await ev.Next())["text"]!.GetValue<string>());
    }

    [Fact]
    public async Task Push_RoutesResultsAndFrames_DropsDuplicates()
    {
        var c = Machine();
        var session = await OpenSession(c);
        await using var ev = await Events.Open(c, session);
        var m = await Linked();

        // A call: the hub sends it down, the machine answers in a pushed segment.
        var call = m.CallAsync("pane.read", new { pane_id = "p1" });
        var cmd = await ev.Next();
        Assert.Equal("call", cmd["type"]!.GetValue<string>());
        var id = cmd["id"]!.GetValue<string>();
        Assert.Equal(HttpStatusCode.NoContent, (await Push(c, session, 1,
            new { type = "result", id, result = new { read = new { text = "hello" } } })).StatusCode);
        Assert.Equal("hello", (await call)!["read"]!["text"]!.GetValue<string>());

        // Frames reach the pane's viewer; a retried segment (same seq) is ignored.
        var frames = new List<string>();
        await using var watch = m.Watch("p1", b64 => { lock (frames) frames.Add(Encoding.UTF8.GetString(Convert.FromBase64String(b64))); return Task.CompletedTask; },
            _ => Task.CompletedTask);
        Assert.Equal("subscribe", (await ev.Next())["type"]!.GetValue<string>());
        var frame = new { type = "frame", pane_id = "p1", bytes = Convert.ToBase64String("out\r\n"u8.ToArray()) };
        await Push(c, session, 2, frame, new { type = "pane_size", pane_id = "p1", cols = 100, rows = 30 });
        var dup = await Push(c, session, 2, frame);
        Assert.True(JsonDocument.Parse(await dup.Content.ReadAsStringAsync()).RootElement.GetProperty("duplicate").GetBoolean());
        // UTF-8 output comes as "text" (no base64).
        await Push(c, session, 3, new { type = "frame", pane_id = "p1", text = "m\u00e1s\r\n" });
        for (var i = 0; i < 100 && frames.Count < 2; i++) await Task.Delay(20);
        await Task.Delay(100);
        Assert.Equal(new[] { "out\r\n", "m\u00e1s\r\n" }, frames);
        Assert.Equal((100, 30), m.PaneSize("p1"));
    }

    [Fact]
    public async Task Machine_GoesOffline_WhenItsStreamIsGone()
    {
        Registry.OfflineAfter = TimeSpan.FromMilliseconds(300);
        var c = Machine();
        var session = await OpenSession(c);
        var ev = await Events.Open(c, session);
        await Linked();
        Registry.Sweep();
        Assert.NotNull(Registry.Get("box")); // streaming: online however long
        await ev.DisposeAsync();
        await Task.Delay(700);
        Registry.Sweep();
        Assert.Null(Registry.Get("box"));
        Assert.Equal(HttpStatusCode.NotFound, (await Push(c, session, 1, new { type = "pong" })).StatusCode);
    }

    [Fact]
    public async Task NewSession_ReplacesTheOld_AndRevokeKillsIt()
    {
        var (token, _) = _factory.Services.GetRequiredService<HubAccounts>().CreateMachine("laptop", "admin");
        var c = Machine(token!);
        var first = await OpenSession(c);
        await using var ev = await Events.Open(c, first);
        var second = await OpenSession(c);
        Assert.NotEqual(first, second);
        // The old stream ends; its session is gone.
        await Assert.ThrowsAsync<EndOfStreamException>(() => ev.Next(TimeSpan.FromSeconds(5)));
        Assert.Equal(HttpStatusCode.NotFound, (await Push(c, first, 1, new { type = "pong" })).StatusCode);
        Assert.Equal(HttpStatusCode.NoContent, (await Push(c, second, 1, new { type = "pong" })).StatusCode);

        _factory.Services.GetRequiredService<HubAccounts>().RevokeMachine("laptop");
        await Registry.DisconnectAsync("laptop");
        Assert.Null(Registry.Get("laptop"));
        Assert.Equal(HttpStatusCode.NotFound, (await Push(c, second, 2, new { type = "pong" })).StatusCode);
        Assert.Equal(HttpStatusCode.Unauthorized, (await c.PostAsJsonAsync("/agent/session", Hello())).StatusCode);
    }
}
