using System.Security.Claims;
using KlarkHub.Components;
using KlarkHub.Services;
using Microsoft.AspNetCore.Authentication;
using Microsoft.AspNetCore.Authentication.Cookies;
using Microsoft.AspNetCore.Identity;
using Microsoft.AspNetCore.Mvc;
using Microsoft.Extensions.Options;
using System.Threading.RateLimiting;
using Microsoft.AspNetCore.RateLimiting;
using Microsoft.AspNetCore.Authentication.JwtBearer;
using Microsoft.AspNetCore.Authorization;
using System.Text.Json;

// Small admin helpers, so no secret ever has to be written to config in clear text.
if (args.Length == 2 && args[0] == "hash-password")
{
    Console.WriteLine(new PasswordHasher<string>().HashPassword("", args[1]));
    return;
}
if (args.Length == 2 && args[0] == "hash-token")
{
    Console.WriteLine(TokenHash.Sha256Hex(args[1]));
    return;
}

var builder = WebApplication.CreateBuilder(args);
// Environment settings use the KLARK_ prefix: KLARK_HUB__AUTH__MODE=Ldap is Hub:Auth:Mode.
// Command-line arguments are added again so they still win over the environment.
builder.Configuration.AddEnvironmentVariables("KLARK_");
builder.Configuration.AddCommandLine(args);

builder.Services.Configure<HubOptions>(builder.Configuration.GetSection("Hub"));
builder.Services.AddSingleton<MockLdapDirectory>();
builder.Services.AddSingleton<LdapDirectory>();
builder.Services.AddSingleton<LocalDirectory>();
builder.Services.AddSingleton<SignInService>();
builder.Services.AddSingleton<HubAccess>();
builder.Services.AddSingleton<HubStore>();
builder.Services.AddHostedService(sp => sp.GetRequiredService<HubStore>());
builder.Services.AddSingleton<MachineRegistry>();
builder.Services.AddSingleton<JwtIssuer>();
builder.Services.AddSingleton<Messaging>();
builder.Services.AddSingleton<IPostConfigureOptions<JwtBearerOptions>, JwtBearerSetup>();
builder.Services.ConfigureHttpJsonOptions(o => o.SerializerOptions.PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower);
builder.Services.AddResponseCompression();
builder.Services.AddRazorComponents().AddInteractiveServerComponents();
builder.Services.AddCascadingAuthenticationState();
builder.Services
    .AddAuthentication(CookieAuthenticationDefaults.AuthenticationScheme)
    .AddCookie(o =>
    {
        o.LoginPath = "/login";
        o.ExpireTimeSpan = TimeSpan.FromDays(14);
        o.SlidingExpiration = true;
        o.Cookie.HttpOnly = true;
        o.Cookie.SameSite = SameSiteMode.Lax;
        // The hub is served over plain HTTP, so the cookie cannot require HTTPS.
        o.Cookie.SecurePolicy = CookieSecurePolicy.SameAsRequest;
    })
    // /api/* (ADR 0009): Bearer user JWT, or Bearer machine token. Pages keep the cookie.
    .AddJwtBearer(ApiAuth.JwtScheme, _ => { })
    .AddScheme<AuthenticationSchemeOptions, MachineTokenHandler>(ApiAuth.MachineScheme, _ => { })
    .AddPolicyScheme(ApiAuth.Scheme, "JWT or machine token", o => o.ForwardDefaultSelector = ApiAuth.SelectScheme);
builder.Services.AddAuthorization(o => o.AddPolicy(ApiAuth.Scheme, p => p
    .AddAuthenticationSchemes(ApiAuth.Scheme).RequireAuthenticatedUser()));

// STATUS S-11: at most 10 sign-in attempts per minute per client address.
builder.Services.AddRateLimiter(o =>
{
    o.RejectionStatusCode = StatusCodes.Status429TooManyRequests;
    o.AddPolicy("login", ctx => RateLimitPartition.GetFixedWindowLimiter(
        ctx.Connection.RemoteIpAddress?.ToString() ?? "unknown",
        _ => new FixedWindowRateLimiterOptions { PermitLimit = 10, Window = TimeSpan.FromMinutes(1), QueueLimit = 0 }));
});

var app = builder.Build();

{
    var auth = app.Services.GetRequiredService<IOptionsMonitor<HubOptions>>().CurrentValue.Auth;
    var log = app.Services.GetRequiredService<ILoggerFactory>().CreateLogger("KlarkHub.Auth");
    log.LogInformation("sign-in mode: {Mode}", auth.Mode);
    if (auth.Mode.Equals("Ldap", StringComparison.OrdinalIgnoreCase))
    {
        log.LogInformation("ldap: {Host}:{Port} ssl={Ssl} starttls={StartTls} {How}", auth.Ldap.Host,
            auth.Ldap.Port > 0 ? auth.Ldap.Port : (auth.Ldap.UseSsl ? 636 : 389), auth.Ldap.UseSsl, auth.Ldap.StartTls,
            string.IsNullOrWhiteSpace(auth.Ldap.UserSearchBase) ? $"bind as {auth.Ldap.UserDnTemplate}" : $"search {auth.Ldap.UserSearchBase} {auth.Ldap.UserFilter}");
    }
    else if (!auth.Mode.Equals("Local", StringComparison.OrdinalIgnoreCase) && !auth.Mode.Equals("MockLdap", StringComparison.OrdinalIgnoreCase))
    {
        throw new InvalidOperationException($"Hub:Auth:Mode must be Ldap, Local or MockLdap, not '{auth.Mode}'");
    }
}

if (!app.Environment.IsDevelopment())
{
    // Plain HTTP by design (ADR 0001, D9): no HSTS, no HTTPS redirect.
    app.UseExceptionHandler("/error", createScopeForErrors: true);
}

app.UseResponseCompression();
app.UseWebSockets(new WebSocketOptions { KeepAliveInterval = TimeSpan.FromSeconds(20) });
app.UseRateLimiter();
app.UseAuthentication();
app.UseAuthorization();
app.UseAntiforgery();
app.MapStaticAssets();

// klark machines dial in here: `klark link --hub ws://<host>:8080/agent --token <token>`.
app.Map("/agent", async (HttpContext ctx, MachineRegistry registry, IOptionsMonitor<HubOptions> options) =>
{
    if (!ctx.WebSockets.IsWebSocketRequest)
    {
        return Results.BadRequest("websocket required");
    }
    var auth = ctx.Request.Headers.Authorization.ToString();
    var token = auth.StartsWith("Bearer ", StringComparison.OrdinalIgnoreCase) ? auth["Bearer ".Length..].Trim() : "";
    var machine = token.Length == 0 ? null : TokenHash.Match(options.CurrentValue.Machines, token);
    if (machine is null)
    {
        return Results.Unauthorized();
    }
    var (machineId, error) = ApiAuth.ResolveMachine(machine, ctx.Request);
    if (machineId is null) return Results.BadRequest(error);
    using var socket = await ctx.WebSockets.AcceptWebSocketAsync();
    await registry.RunAsync(socket, machineId, ctx.RequestAborted);
    return Results.Empty;
});

app.MapPost("/account/login", async (HttpContext ctx, [FromForm] string username, [FromForm] string password,
    [FromForm] string? returnUrl, SignInService signIn) =>
{
    var user = await signIn.SignInAsync(username, password, ctx.RequestAborted);
    if (user is null)
    {
        return Results.Redirect("/login?failed=1");
    }
    var claims = new List<Claim>
    {
        new(ClaimTypes.Name, user.Name),
        new("display_name", user.DisplayName),
    };
    claims.AddRange(user.Groups.Select(g => new Claim(ClaimTypes.Role, g)));
    var identity = new ClaimsIdentity(claims, CookieAuthenticationDefaults.AuthenticationScheme);
    await ctx.SignInAsync(CookieAuthenticationDefaults.AuthenticationScheme, new ClaimsPrincipal(identity));
    var target = !string.IsNullOrEmpty(returnUrl) && returnUrl.StartsWith('/') && !returnUrl.StartsWith("//") ? returnUrl : "/";
    return Results.LocalRedirect(target);
}).RequireRateLimiting("login");


app.MapPost("/account/logout", async (HttpContext ctx) =>
{
    await ctx.SignOutAsync(CookieAuthenticationDefaults.AuthenticationScheme);
    return Results.Redirect("/login");
}).RequireAuthorization();

// ---- HTTP API (ADR 0009) ----------------------------------------------------

// A user JWT for the CLI: `klark msg login`. Same directory and rate limit as the login form.
app.MapPost("/api/token", async (HttpContext ctx, SignInService signIn, JwtIssuer jwt) =>
{
    string? username = null, password = null;
    if (ctx.Request.HasFormContentType)
    {
        var form = await ctx.Request.ReadFormAsync(ctx.RequestAborted);
        (username, password) = (form["username"], form["password"]);
    }
    else
    {
        try
        {
            var body = await ctx.Request.ReadFromJsonAsync<TokenRequest>(ctx.RequestAborted);
            (username, password) = (body?.Username, body?.Password);
        }
        catch (JsonException) { }
    }
    if (string.IsNullOrEmpty(username) || string.IsNullOrEmpty(password))
    {
        return Results.BadRequest(new { error = "username and password are required" });
    }
    var user = await signIn.SignInAsync(username, password, ctx.RequestAborted);
    if (user is null)
    {
        return Results.Json(new { error = "sign-in failed" }, statusCode: StatusCodes.Status401Unauthorized);
    }
    var (token, expires) = jwt.Issue(user.Name, user.DisplayName, user.Groups);
    return Results.Ok(new { token, expires_at = expires, user = user.Name, groups = user.Groups });
}).RequireRateLimiting("login").DisableAntiforgery();

var api = app.MapGroup("/api").RequireAuthorization(ApiAuth.Scheme);
api.MapGet("/agents", (HttpContext ctx, Messaging messaging) => messaging.ListAgentsAsync(ctx.User));
api.MapPost("/messages", async (HttpContext ctx, SendRequest body, Messaging messaging) =>
{
    var r = await messaging.SendAsync(ctx.User, body.To, body.Text, body.From);
    return r.Status == 200
        ? Results.Ok(new { id = r.Id, delivered = true })
        : Results.Json(new { id = r.Id, error = r.Error }, statusCode: r.Status);
});
api.MapGet("/messages", async (HttpContext ctx, Messaging messaging, int? limit) =>
    (await messaging.RecentAsync(ctx.User, limit ?? 100)).Select(m => new
    {
        id = m.Id, ts = m.Ts, from = m.From, to = m.To, text = m.Text, sender = m.Sender, ok = m.Ok, error = m.Error,
    }));

app.MapGet("/healthz", () => Results.Ok("ok"));

app.MapRazorComponents<App>().AddInteractiveServerRenderMode();

app.Run();

record TokenRequest(string? Username, string? Password);
record SendRequest(string? To, string? Text, string? From);
