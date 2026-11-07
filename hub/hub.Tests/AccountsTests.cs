using System.Net;
using System.Net.Http.Json;
using System.Text.Json;
using System.Text.RegularExpressions;
using KlarkHub.Services;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Mvc.Testing;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Microsoft.Extensions.Options;

namespace KlarkHub.Tests;

/// <summary>A data directory that disappears after the test.</summary>
public sealed class TempDir : IDisposable
{
    public string Path { get; } = Directory.CreateTempSubdirectory("klark-hub-test-").FullName;
    public void Dispose()
    {
        try { Directory.Delete(Path, true); } catch (IOException) { }
    }
}

public class HubAccountsTests : IDisposable
{
    private readonly TempDir _dir = new();
    public void Dispose() => _dir.Dispose();

    private HubAccounts Accounts(Action<HubOptions>? configure = null)
    {
        var o = new HubOptions { DataDir = _dir.Path };
        o.Auth.Mode = "MockLdap";
        configure?.Invoke(o);
        return new HubAccounts(new StaticOptions(o), NullLogger<HubAccounts>.Instance);
    }

    [Fact]
    public void FirstStart_SeedsAdminWithDefaultPassword_Hashed()
    {
        var a = Accounts();
        Assert.True(a.VerifyAdmin(HubAccounts.DefaultAdminPassword));
        Assert.True(a.AdminPasswordIsDefault());
        Assert.False(a.VerifyAdmin("wrong"));
        // Never stored in clear text.
        Assert.DoesNotContain(HubAccounts.DefaultAdminPassword, File.ReadAllText(System.IO.Path.Combine(_dir.Path, "hub.db")));
    }

    [Fact]
    public void ConfiguredAdminPassword_IsNotDefault()
    {
        var a = Accounts(o => o.Admin.Password = "s3cret-from-env");
        Assert.True(a.VerifyAdmin("s3cret-from-env"));
        Assert.False(a.VerifyAdmin(HubAccounts.DefaultAdminPassword));
        Assert.False(a.AdminPasswordIsDefault());
    }

    [Fact]
    public void AdminPasswordChange_RequiresCurrent_AndClearsDefaultFlag()
    {
        var a = Accounts();
        Assert.NotNull(a.ChangeOwnPassword("admin", "nope-nope", "new-password-1"));
        Assert.True(a.AdminPasswordIsDefault());
        Assert.NotNull(a.ChangeOwnPassword("admin", HubAccounts.DefaultAdminPassword, "short"));
        Assert.Null(a.ChangeOwnPassword("admin", HubAccounts.DefaultAdminPassword, "new-password-1"));
        Assert.False(a.AdminPasswordIsDefault());
        Assert.True(a.VerifyAdmin("new-password-1"));
        Assert.False(a.VerifyAdmin(HubAccounts.DefaultAdminPassword));
    }

    [Fact]
    public void MockUsers_SeededAndEditable()
    {
        var a = Accounts();
        Assert.Equal(new[] { "user1", "user2" }, a.MockUsers().Select(u => u.Uid));
        Assert.NotNull(a.VerifyMockUser("user1", "user1pass"));
        Assert.Null(a.VerifyMockUser("user1", "user2pass"));

        Assert.Null(a.AddMockUser("carol", "Carol", new[] { "klark-users", "ops" }, "carol-pass"));
        Assert.NotNull(a.AddMockUser("carol", "Carol", new[] { "x" }, "carol-pass")); // duplicate
        Assert.NotNull(a.AddMockUser("admin", "", new[] { "x" }, "whatever-1")); // reserved
        var carol = a.VerifyMockUser("CAROL", "carol-pass");
        Assert.Equal(new[] { "klark-users", "ops" }, carol!.Groups);
        Assert.Equal(DirectoryUser.Mock, carol.Source);

        Assert.Null(a.ResetMockPassword("carol", "carol-new-pass"));
        Assert.Null(a.VerifyMockUser("carol", "carol-pass"));
        Assert.NotNull(a.VerifyMockUser("carol", "carol-new-pass"));

        Assert.NotNull(a.ChangeOwnPassword("carol", "wrong-pass", "carol-third"));
        Assert.Null(a.ChangeOwnPassword("carol", "carol-new-pass", "carol-third"));
        Assert.NotNull(a.VerifyMockUser("carol", "carol-third"));

        Assert.True(a.DeleteMockUser("carol"));
        Assert.Null(a.VerifyMockUser("carol", "carol-third"));
        Assert.False(a.DeleteMockUser("carol"));
    }

    [Fact]
    public void ConfiguredMockUsers_AreTheSeed()
    {
        var a = Accounts(o => o.Auth.MockUsers.Add(new MockLdapEntry { Uid = "dana", Password = "dana-pass", Groups = { "g" } }));
        Assert.Equal(new[] { "dana" }, a.MockUsers().Select(u => u.Uid));
        Assert.NotNull(a.VerifyMockUser("dana", "dana-pass"));
    }

    [Fact]
    public void Mode_StoredOverridesConfiguration_AndLdapBindPasswordIsKept()
    {
        var a = Accounts(o => o.Auth.Ldap.BindPassword = "from-config");
        Assert.Equal("MockLdap", a.EffectiveMode());
        Assert.False(a.LdapIsStored());
        Assert.Null(a.SaveMode("ldap"));
        Assert.Equal("Ldap", a.EffectiveMode());
        Assert.NotNull(a.SaveMode("bogus"));
        Assert.Null(a.SaveLdap(new LdapOptions { Host = "ldap.example.com", Port = 636, UseSsl = true }));
        var l = a.EffectiveLdap();
        Assert.Equal("ldap.example.com", l.Host);
        Assert.Equal("from-config", l.BindPassword);
        Assert.True(a.LdapIsStored());
    }

    [Fact]
    public void Machines_CreateMatchRevoke()
    {
        var a = Accounts(o => o.Machines.Add(new HubMachine { Name = "cfg", TokenSha256 = TokenHash.Sha256Hex("cfg-token") }));
        var (token, error) = a.CreateMachine("laptop", "user1");
        Assert.Null(error);
        Assert.Equal("laptop", a.MatchToken(token!)?.Name);
        Assert.Equal("cfg", a.MatchToken("cfg-token")?.Name); // configuration still works
        Assert.Null(a.MatchToken("nope"));
        Assert.DoesNotContain(token!, File.ReadAllText(System.IO.Path.Combine(_dir.Path, "hub.db")));
        Assert.NotNull(a.CreateMachine("cfg", "user1").Error);
        Assert.NotNull(a.CreateMachine("laptop", "user2", approverIsAdmin: false).Error); // someone else's
        Assert.True(a.RevokeMachine("laptop"));
        Assert.Null(a.MatchToken(token!));
    }

    [Fact]
    public void ConnectFlow_ExpiresAndIsSingleUse()
    {
        var a = Accounts();
        var connect = new MachineConnect(a, NullLogger<MachineConnect>.Instance);
        var now = DateTimeOffset.UtcNow;
        connect.Now = () => now;
        var (p, _) = connect.Start("box");
        Assert.Matches("^[A-Z2-9]{4}-[A-Z2-9]{4}$", p!.Code);
        Assert.Equal("pending", connect.Poll(p.PollId).Status);
        now += MachineConnect.Lifetime + TimeSpan.FromSeconds(1);
        Assert.Null(connect.Find(p.Code));
        Assert.NotNull(connect.Decide(p.Code, true, "user1", false));
        Assert.Equal("expired", connect.Poll(p.PollId).Status);
        Assert.Equal("unknown", connect.Poll(p.PollId).Status);
        Assert.Equal("unknown", connect.Poll("nope").Status);
        Assert.NotNull(connect.Start("bad name!").Error);
    }

    private sealed class StaticOptions(HubOptions value) : IOptionsMonitor<HubOptions>
    {
        public HubOptions CurrentValue => value;
        public HubOptions Get(string? name) => value;
        public IDisposable? OnChange(Action<HubOptions, string?> listener) => null;
    }
}

/// <summary>The real hub in memory: sign-in, admin pages, and the device flow over HTTP.</summary>
public class HubAccountsHttpTests : IDisposable
{
    private readonly TempDir _dir = new();
    private readonly WebApplicationFactory<Program> _factory;

    public HubAccountsHttpTests()
    {
        _factory = new WebApplicationFactory<Program>().WithWebHostBuilder(b =>
        {
            b.UseSetting("Hub:DataDir", _dir.Path);
            b.UseSetting("Hub:Auth:Mode", "");
            b.UseSetting("Hub:Auth:Ldap:Host", "");
            b.UseEnvironment("Production");
        });
    }

    public void Dispose()
    {
        _factory.Dispose();
        _dir.Dispose();
    }

    private HttpClient Client() => _factory.CreateClient(new WebApplicationFactoryClientOptions { AllowAutoRedirect = false });

    private static async Task<string> TokenFor(HttpClient c, string page)
    {
        var html = await c.GetStringAsync(page);
        return Regex.Match(html, "name=\"__RequestVerificationToken\"[^>]*value=\"([^\"]+)\"").Groups[1].Value;
    }

    private static async Task<HttpResponseMessage> Post(HttpClient c, string page, string action, params (string, string)[] fields)
    {
        var form = fields.Select(f => new KeyValuePair<string, string>(f.Item1, f.Item2)).ToList();
        form.Add(new("__RequestVerificationToken", await TokenFor(c, page)));
        return await c.PostAsync(action, new FormUrlEncodedContent(form));
    }

    private async Task<HttpClient> SignedIn(string user, string password)
    {
        var c = Client();
        var r = await Post(c, "/login", "/account/login", ("username", user), ("password", password), ("returnUrl", "/"));
        Assert.Equal(HttpStatusCode.Redirect, r.StatusCode);
        Assert.DoesNotContain("failed", r.Headers.Location!.ToString());
        return c;
    }

    private async Task<bool> CanSignIn(string user, string password)
    {
        var c = Client();
        var r = await Post(c, "/login", "/account/login", ("username", user), ("password", password), ("returnUrl", "/"));
        return !r.Headers.Location!.ToString().Contains("failed");
    }

    [Fact]
    public async Task FreshHub_IsMockLdap_AdminSeesDefaultBanner()
    {
        Assert.Equal("MockLdap", _factory.Services.GetRequiredService<HubAccounts>().EffectiveMode());
        var admin = await SignedIn("admin", "adminadmin");
        Assert.Contains("Change the default admin password", await admin.GetStringAsync("/account/password"));
        var user = await SignedIn("user1", "user1pass");
        Assert.DoesNotContain("Change the default admin password", await user.GetStringAsync("/account/password"));
    }

    [Fact]
    public async Task PasswordChange_WrongCurrentRejected_ThenBannerGoes()
    {
        var admin = await SignedIn("admin", "adminadmin");
        var r = await Post(admin, "/account/password", "/account/password", ("current", "wrong"), ("next", "brand-new-pw"), ("confirm", "brand-new-pw"));
        Assert.Contains("error=", r.Headers.Location!.ToString());
        r = await Post(admin, "/account/password", "/account/password", ("current", "adminadmin"), ("next", "brand-new-pw"), ("confirm", "brand-new-pw"));
        Assert.Contains("ok=", r.Headers.Location!.ToString());
        Assert.DoesNotContain("Change the default admin password", await admin.GetStringAsync("/account/password"));
        Assert.True(await CanSignIn("admin", "brand-new-pw"));
        Assert.False(await CanSignIn("admin", "adminadmin"));
    }

    [Fact]
    public async Task Forms_RequireAntiforgery()
    {
        var admin = await SignedIn("admin", "adminadmin");
        var r = await admin.PostAsync("/admin/users/add", new FormUrlEncodedContent(new Dictionary<string, string>
        {
            ["uid"] = "eve", ["password"] = "eve-password",
        }));
        Assert.Equal(HttpStatusCode.BadRequest, r.StatusCode);
    }

    [Fact]
    public async Task NonAdmin_ForbiddenOnAdminPagesAndPosts()
    {
        var user = await SignedIn("user1", "user1pass");
        foreach (var page in new[] { "/admin/users", "/admin/directory", "/admin/machines" })
        {
            Assert.Equal(HttpStatusCode.Forbidden, (await user.GetAsync(page)).StatusCode);
        }
        var token = await TokenFor(user, "/account/password");
        var r = await user.PostAsync("/admin/directory/mode", new FormUrlEncodedContent(new Dictionary<string, string>
        {
            ["mode"] = "Ldap", ["__RequestVerificationToken"] = token,
        }));
        Assert.Equal(HttpStatusCode.Forbidden, r.StatusCode);
        Assert.Equal("MockLdap", _factory.Services.GetRequiredService<HubAccounts>().EffectiveMode());
        // Signed out: sent to sign in.
        Assert.Equal(HttpStatusCode.Redirect, (await Client().GetAsync("/admin/users")).StatusCode);
    }

    [Fact]
    public async Task Admin_AddsUser_WhoCanSignIn_AndModeSwitchTakesEffect()
    {
        var admin = await SignedIn("admin", "adminadmin");
        Assert.Contains("user1", await admin.GetStringAsync("/admin/users"));
        var r = await Post(admin, "/admin/users", "/admin/users/add", ("uid", "carol"), ("display", "Carol"), ("groups", "klark-users"), ("password", "carol-pass"));
        Assert.Contains("ok=", r.Headers.Location!.ToString());
        Assert.True(await CanSignIn("carol", "carol-pass"));

        await Post(admin, "/admin/users", "/admin/users/delete", ("uid", "carol"));
        Assert.False(await CanSignIn("carol", "carol-pass"));

        // Ldap with an unreachable server: mock users stop working, the admin still signs in.
        await Post(admin, "/admin/directory", "/admin/directory/ldap", ("host", "127.0.0.1"), ("port", "1"), ("tls", "none"));
        r = await Post(admin, "/admin/directory", "/admin/directory/mode", ("mode", "Ldap"));
        Assert.Contains("ok=", r.Headers.Location!.ToString());
        Assert.False(await CanSignIn("user1", "user1pass"));
        Assert.True(await CanSignIn("admin", "adminadmin"));
        await Post(admin, "/admin/directory", "/admin/directory/mode", ("mode", "MockLdap"));
        Assert.True(await CanSignIn("user1", "user1pass"));
    }

    private static async Task<JsonElement> Json(HttpResponseMessage r) => JsonDocument.Parse(await r.Content.ReadAsStringAsync()).RootElement;

    [Fact]
    public async Task ConnectFlow_ApproveGivesTokenOnce_WhichAuthenticates()
    {
        var machine = Client();
        var start = await Json(await machine.PostAsJsonAsync("/api/connect/start", new { name = "laptop" }));
        var code = start.GetProperty("code").GetString()!;
        var pollId = start.GetProperty("poll_id").GetString()!;
        Assert.EndsWith("/connect?code=" + code, start.GetProperty("url").GetString());
        Assert.Equal(600, start.GetProperty("expires_in").GetInt32());

        var poll = await Json(await machine.PostAsJsonAsync("/api/connect/poll", new { poll_id = pollId }));
        Assert.Equal("pending", poll.GetProperty("status").GetString());

        var user = await SignedIn("user1", "user1pass");
        Assert.Contains(code, await user.GetStringAsync("/connect?code=" + code));
        var r = await Post(user, "/connect?code=" + code, "/connect/decide", ("code", code), ("decision", "approve"));
        Assert.Contains("ok=", r.Headers.Location!.ToString());

        poll = await Json(await machine.PostAsJsonAsync("/api/connect/poll", new { poll_id = pollId }));
        Assert.Equal("approved", poll.GetProperty("status").GetString());
        Assert.Equal("laptop", poll.GetProperty("machine").GetString());
        var token = poll.GetProperty("token").GetString()!;
        var again = await machine.PostAsJsonAsync("/api/connect/poll", new { poll_id = pollId });
        Assert.Equal(HttpStatusCode.NotFound, again.StatusCode);

        // The token is a machine token on /api ...
        var api = Client();
        api.DefaultRequestHeaders.Authorization = new("Bearer", token);
        Assert.Equal(HttpStatusCode.OK, (await api.GetAsync("/api/messages")).StatusCode);
        // ... and opens a link session (ADR 0013).
        var session = await api.PostAsJsonAsync("/agent/session",
            new { type = "hello", protocol = 4, machine_id = "laptop", machine_name = "laptop", klark_version = "t" });
        Assert.Equal(HttpStatusCode.OK, session.StatusCode);

        var accounts = _factory.Services.GetRequiredService<HubAccounts>();
        Assert.Equal("user1", accounts.MachineRows().Single().ApprovedBy);
        // Revoked: the token stops working.
        var admin = await SignedIn("admin", "adminadmin");
        await Post(admin, "/admin/machines", "/admin/machines/revoke", ("name", "laptop"));
        Assert.Equal(HttpStatusCode.Unauthorized, (await api.GetAsync("/api/messages")).StatusCode);
    }

    [Fact]
    public async Task ConnectFlow_DenyAndUnknown()
    {
        var machine = Client();
        var start = await Json(await machine.PostAsJsonAsync("/api/connect/start", new { name = "box" }));
        var user = await SignedIn("user2", "user2pass");
        var code = start.GetProperty("code").GetString()!;
        await Post(user, "/connect?code=" + code, "/connect/decide", ("code", code), ("decision", "deny"));
        var poll = await Json(await machine.PostAsJsonAsync("/api/connect/poll", new { poll_id = start.GetProperty("poll_id").GetString() }));
        Assert.Equal("denied", poll.GetProperty("status").GetString());
        Assert.Equal(HttpStatusCode.NotFound, (await machine.PostAsJsonAsync("/api/connect/poll", new { poll_id = "nope" })).StatusCode);
        Assert.Equal(HttpStatusCode.BadRequest, (await machine.PostAsJsonAsync("/api/connect/start", new { name = "" })).StatusCode);
        Assert.Empty(_factory.Services.GetRequiredService<HubAccounts>().MachineRows());
    }
}
