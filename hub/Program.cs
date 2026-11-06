using System.Security.Claims;
using KlarkHub.Components;
using KlarkHub.Services;
using Microsoft.AspNetCore.Authentication;
using Microsoft.AspNetCore.Authentication.Cookies;
using Microsoft.AspNetCore.Identity;
using Microsoft.AspNetCore.Mvc;
using Microsoft.Extensions.Options;

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

builder.Services.Configure<HubOptions>(builder.Configuration.GetSection("Hub"));
builder.Services.AddSingleton<HubStore>();
builder.Services.AddHostedService(sp => sp.GetRequiredService<HubStore>());
builder.Services.AddSingleton<MachineRegistry>();
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
    });
builder.Services.AddAuthorization();

var app = builder.Build();

if (!app.Environment.IsDevelopment())
{
    // Plain HTTP by design (ADR 0001, D9): no HSTS, no HTTPS redirect.
    app.UseExceptionHandler("/error", createScopeForErrors: true);
}

app.UseResponseCompression();
app.UseWebSockets(new WebSocketOptions { KeepAliveInterval = TimeSpan.FromSeconds(20) });
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
    using var socket = await ctx.WebSockets.AcceptWebSocketAsync();
    await registry.RunAsync(socket, machine, ctx.RequestAborted);
    return Results.Empty;
});

app.MapPost("/account/login", async (HttpContext ctx, [FromForm] string username, [FromForm] string password,
    [FromForm] string? returnUrl, IOptionsMonitor<HubOptions> options) =>
{
    var user = options.CurrentValue.Users.FirstOrDefault(u => string.Equals(u.Name, username, StringComparison.OrdinalIgnoreCase));
    var ok = user is not null && !string.IsNullOrEmpty(user.PasswordHash) &&
             new PasswordHasher<string>().VerifyHashedPassword("", user.PasswordHash, password) != PasswordVerificationResult.Failed;
    if (!ok)
    {
        return Results.Redirect("/login?failed=1");
    }
    var identity = new ClaimsIdentity(new[] { new Claim(ClaimTypes.Name, user!.Name) }, CookieAuthenticationDefaults.AuthenticationScheme);
    await ctx.SignInAsync(CookieAuthenticationDefaults.AuthenticationScheme, new ClaimsPrincipal(identity));
    var target = !string.IsNullOrEmpty(returnUrl) && returnUrl.StartsWith('/') && !returnUrl.StartsWith("//") ? returnUrl : "/";
    return Results.LocalRedirect(target);
});

app.MapPost("/account/logout", async (HttpContext ctx) =>
{
    await ctx.SignOutAsync(CookieAuthenticationDefaults.AuthenticationScheme);
    return Results.Redirect("/login");
}).RequireAuthorization();

app.MapGet("/healthz", () => Results.Ok("ok"));

app.MapRazorComponents<App>().AddInteractiveServerRenderMode();

app.Run();
