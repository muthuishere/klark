using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.Extensions.Options;

namespace KlarkHub.Services;

/// <summary>
/// Form posts behind /account/password, /admin/* and /connect, plus the device
/// flow API for `klark hub connect` (ADR 0009). Forms carry antiforgery tokens
/// (validated because they bind form data); /admin/* is guarded by middleware.
/// Every handler redirects back to its page with a message in the query string.
/// </summary>
public static class AccountEndpoints
{
    /// <summary>Claim naming where an account lives: "builtin", "mock" or empty (a directory).</summary>
    public const string SourceClaim = "klark_source";

    public static string PublicBase(HttpContext ctx, HubOptions o) =>
        !string.IsNullOrWhiteSpace(o.PublicUrl)
            ? o.PublicUrl.TrimEnd('/')
            : $"{ctx.Request.Scheme}://{ctx.Request.Host}{ctx.Request.PathBase}";

    private static IResult Back(string page, string? error, string? ok = null) =>
        Results.LocalRedirect(page + (page.Contains('?') ? "&" : "?") +
            (error is not null ? "error=" + Uri.EscapeDataString(error) : "ok=" + Uri.EscapeDataString(ok ?? "Saved.")));

    public static void MapAccountEndpoints(this WebApplication app)
    {
        app.MapPost("/account/password", (HttpContext ctx, [FromForm] IFormCollection form, HubAccounts accounts) =>
        {
            const string page = "/account/password";
            var name = ctx.User.Identity?.Name ?? "";
            var source = ctx.User.FindFirst(SourceClaim)?.Value;
            if (source is not (DirectoryUser.BuiltIn or DirectoryUser.Mock))
                return Back(page, "Your password is managed by your directory, not by this hub.");
            if (form["next"] != form["confirm"]) return Back(page, "The new passwords do not match.");
            var error = accounts.ChangeOwnPassword(name, form["current"].ToString(), form["next"].ToString());
            return Back(page, error, "Password changed.");
        }).RequireAuthorization();

        // ---- /admin/users ---------------------------------------------------------
        app.MapPost("/admin/users/add", ([FromForm] IFormCollection form, HubAccounts accounts) =>
            Back("/admin/users", accounts.AddMockUser(form["uid"].ToString(), form["display"].ToString(),
                HubAccounts.SplitGroups(form["groups"]), form["password"].ToString()), $"Added {form["uid"]}."));
        app.MapPost("/admin/users/reset", ([FromForm] IFormCollection form, HubAccounts accounts) =>
            Back("/admin/users", accounts.ResetMockPassword(form["uid"].ToString(), form["password"].ToString()),
                $"Password reset for {form["uid"]}."));
        app.MapPost("/admin/users/delete", ([FromForm] IFormCollection form, HubAccounts accounts) =>
            Back("/admin/users", accounts.DeleteMockUser(form["uid"].ToString()) ? null : "No such user.", $"Deleted {form["uid"]}."));

        // ---- /admin/directory -----------------------------------------------------
        app.MapPost("/admin/directory/mode", ([FromForm] IFormCollection form, HubAccounts accounts) =>
            Back("/admin/directory", accounts.SaveMode(form["mode"].ToString()), $"Sign-in mode is now {form["mode"]}."));
        app.MapPost("/admin/directory/ldap", ([FromForm] IFormCollection form, HubAccounts accounts) =>
        {
            var o = new LdapOptions
            {
                Host = form["host"].ToString().Trim(),
                Port = int.TryParse(form["port"], out var port) ? port : 0,
                UseSsl = form["tls"] == "ssl",
                StartTls = form["tls"] == "starttls",
                SkipCertificateCheck = form["skipcert"] == "on",
                UserDnTemplate = form["userdn"].ToString().Trim(),
                UserSearchBase = form["searchbase"].ToString().Trim(),
                UserFilter = string.IsNullOrWhiteSpace(form["filter"]) ? new LdapOptions().UserFilter : form["filter"].ToString().Trim(),
                BindDn = form["binddn"].ToString().Trim(),
                BindPassword = form["bindpassword"].ToString(),
                GroupSearchBase = form["groupbase"].ToString().Trim(),
                UseMemberOf = form["memberof"] == "on",
                TimeoutSeconds = accounts.EffectiveLdap().TimeoutSeconds,
            };
            return Back("/admin/directory", accounts.SaveLdap(o), "LDAP settings saved.");
        });
        app.MapPost("/admin/directory/test", async ([FromForm] IFormCollection form, HubAccounts accounts, LdapDirectory ldap) =>
        {
            var (ok, message) = await ldap.TestAsync(accounts.EffectiveLdap(), form["testuser"].ToString(), form["testpassword"].ToString());
            return Back("/admin/directory", ok ? null : message, message);
        });

        // ---- /admin/machines ------------------------------------------------------
        app.MapPost("/admin/machines/revoke", async ([FromForm] IFormCollection form, HubAccounts accounts, MachineRegistry registry) =>
        {
            var name = form["name"].ToString();
            if (!accounts.RevokeMachine(name)) return Back("/admin/machines", "No such machine (machines from configuration cannot be revoked here).");
            await registry.DisconnectAsync(name);
            return Back("/admin/machines", null, $"Revoked {name}; its link was dropped.");
        });

        // ---- device flow: `klark hub connect` ------------------------------------------
        app.MapPost("/connect/decide", (HttpContext ctx, [FromForm] IFormCollection form, MachineConnect connect, HubAccess access) =>
        {
            var code = form["code"].ToString();
            var approve = form["decision"] == "approve";
            var error = connect.Decide(code, approve, ctx.User.Identity?.Name ?? "", access.IsAdmin(ctx.User));
            return Back("/connect?code=" + Uri.EscapeDataString(code), error,
                approve ? "Approved. The machine is connecting now." : "Denied.");
        }).RequireAuthorization();

        app.MapPost("/api/connect/start", async (HttpContext ctx, MachineConnect connect, IOptionsMonitor<HubOptions> options) =>
        {
            ConnectStart? body = null;
            try { body = await ctx.Request.ReadFromJsonAsync<ConnectStart>(ctx.RequestAborted); }
            catch (System.Text.Json.JsonException) { }
            catch (InvalidOperationException) { }
            var (p, error) = connect.Start(body?.Name);
            if (p is null) return Results.BadRequest(new { error });
            return Results.Ok(new
            {
                code = p.Code,
                poll_id = p.PollId,
                url = $"{PublicBase(ctx, options.CurrentValue)}/connect?code={p.Code}",
                expires_in = (int)MachineConnect.Lifetime.TotalSeconds,
                interval = MachineConnect.IntervalSeconds,
            });
        }).RequireRateLimiting("connect").DisableAntiforgery();

        app.MapPost("/api/connect/poll", async (HttpContext ctx, MachineConnect connect) =>
        {
            ConnectPoll? body = null;
            try { body = await ctx.Request.ReadFromJsonAsync<ConnectPoll>(ctx.RequestAborted); }
            catch (System.Text.Json.JsonException) { }
            catch (InvalidOperationException) { }
            var (status, token, machine) = connect.Poll(body?.PollId);
            return status switch
            {
                "approved" => Results.Ok(new { status, token, machine }),
                "unknown" => Results.NotFound(new { status }),
                _ => Results.Ok(new { status }),
            };
        }).RequireRateLimiting("connect").DisableAntiforgery();
    }

    private sealed record ConnectStart(string? Name);
    private sealed record ConnectPoll([property: JsonPropertyName("poll_id")] string? PollId);
}
