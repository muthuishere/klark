using System.Security.Claims;
using System.Security.Cryptography;
using System.Text;
using System.Text.Encodings.Web;
using Microsoft.AspNetCore.Authentication;
using Microsoft.Extensions.Options;
using Microsoft.IdentityModel.JsonWebTokens;
using Microsoft.IdentityModel.Tokens;

namespace KlarkHub.Services;

/// <summary>
/// Issues and describes user JWTs (ADR 0009): HS256, claims name, display_name and role (one per group).
/// </summary>
public sealed class JwtIssuer
{
    public const string Issuer = "klark-hub";
    public const string Audience = "klark-hub";

    private readonly IOptionsMonitor<HubOptions> _options;

    public JwtIssuer(IOptionsMonitor<HubOptions> options, ILogger<JwtIssuer> log)
    {
        _options = options;
        var o = options.CurrentValue;
        if (!string.IsNullOrEmpty(o.Jwt.Key))
        {
            var bytes = Encoding.UTF8.GetBytes(o.Jwt.Key);
            if (bytes.Length < 32) throw new InvalidOperationException("Hub:Jwt:Key must be at least 32 bytes");
            Key = new SymmetricSecurityKey(bytes);
            return;
        }
        // No configured key: make one once and keep it, so tokens survive restarts.
        Directory.CreateDirectory(o.DataDir);
        var path = Path.Combine(o.DataDir, "jwt.key");
        if (!File.Exists(path))
        {
            var fresh = Convert.ToBase64String(RandomNumberGenerator.GetBytes(32));
            var create = new FileStreamOptions { Mode = FileMode.CreateNew, Access = FileAccess.Write };
            if (!OperatingSystem.IsWindows()) create.UnixCreateMode = UnixFileMode.UserRead | UnixFileMode.UserWrite;
            using (var f = new FileStream(path, create))
            using (var w = new StreamWriter(f))
            {
                w.Write(fresh);
            }
            log.LogInformation("created a JWT signing key in {Path}", path);
        }
        Key = new SymmetricSecurityKey(Convert.FromBase64String(File.ReadAllText(path).Trim()));
    }

    public SymmetricSecurityKey Key { get; }

    public (string Token, DateTimeOffset ExpiresAt) Issue(string name, string displayName, IEnumerable<string> groups)
    {
        var expires = DateTimeOffset.UtcNow.AddHours(_options.CurrentValue.Jwt.Hours);
        var claims = new List<Claim> { new("name", name), new("display_name", displayName) };
        claims.AddRange(groups.Select(g => new Claim("role", g)));
        var token = new JsonWebTokenHandler().CreateToken(new SecurityTokenDescriptor
        {
            Subject = new ClaimsIdentity(claims),
            Issuer = Issuer,
            Audience = Audience,
            Expires = expires.UtcDateTime,
            SigningCredentials = new SigningCredentials(Key, SecurityAlgorithms.HmacSha256),
        });
        return (token, expires);
    }

    public (string Token, DateTimeOffset ExpiresAt) Issue(ClaimsPrincipal user) =>
        Issue(user.Identity?.Name ?? "", user.FindFirst("display_name")?.Value ?? user.Identity?.Name ?? "",
            user.Claims.Where(c => c.Type == ClaimTypes.Role || c.Type == "role").Select(c => c.Value).Distinct());

    public TokenValidationParameters Validation() => new()
    {
        ValidIssuer = Issuer,
        ValidAudience = Audience,
        IssuerSigningKey = Key,
        NameClaimType = "name",
        RoleClaimType = "role",
        ClockSkew = TimeSpan.FromMinutes(1),
    };
}

/// <summary>Configures the JwtBearer scheme from <see cref="JwtIssuer"/> once DI is built.</summary>
public sealed class JwtBearerSetup(JwtIssuer issuer) : IPostConfigureOptions<Microsoft.AspNetCore.Authentication.JwtBearer.JwtBearerOptions>
{
    public void PostConfigure(string? name, Microsoft.AspNetCore.Authentication.JwtBearer.JwtBearerOptions options)
    {
        if (name != ApiAuth.JwtScheme) return;
        options.MapInboundClaims = false;
        options.TokenValidationParameters = issuer.Validation();
    }
}

public static class ApiAuth
{
    public const string Scheme = "Api";
    public const string JwtScheme = "Jwt";
    public const string MachineScheme = "MachineToken";

    /// <summary>Claim carried by a caller that presented a machine token; its value is the machine id.</summary>
    public const string MachineClaim = "klark_machine";

    public static string? MachineOf(ClaimsPrincipal user) => user.FindFirst(MachineClaim)?.Value;

    /// <summary>A JWT has two dots; a machine token (hex / random string) does not.</summary>
    public static string SelectScheme(HttpContext ctx)
    {
        var auth = ctx.Request.Headers.Authorization.ToString();
        var token = auth.StartsWith("Bearer ", StringComparison.OrdinalIgnoreCase) ? auth["Bearer ".Length..].Trim() : "";
        return token.Count(c => c == '.') == 2 ? JwtScheme : MachineScheme;
    }

    /// <summary>
    /// The machine id a token stands for, as /agent names it: the entry's name,
    /// or "{name}:{X-Klark-Machine}" for fleet tokens. Null with an error when refused.
    /// </summary>
    public static (string? Id, string? Error) ResolveMachine(HubMachine machine, HttpRequest request)
    {
        if (!machine.Fleet) return (machine.Name, null);
        var member = new string(request.Headers["X-Klark-Machine"].ToString()
            .Where(c => char.IsAsciiLetterOrDigit(c) || c is '-' or '_' or '.').Take(64).ToArray());
        return member.Length == 0 ? (null, "fleet tokens need an X-Klark-Machine header") : ($"{machine.Name}:{member}", null);
    }
}

/// <summary>`Authorization: Bearer &lt;machine token&gt;` on /api: the caller acts as that machine (ADR 0009).</summary>
public sealed class MachineTokenHandler(
    IOptionsMonitor<AuthenticationSchemeOptions> options, ILoggerFactory logger, UrlEncoder encoder,
    IOptionsMonitor<HubOptions> hub) : AuthenticationHandler<AuthenticationSchemeOptions>(options, logger, encoder)
{
    protected override Task<AuthenticateResult> HandleAuthenticateAsync()
    {
        var auth = Request.Headers.Authorization.ToString();
        if (!auth.StartsWith("Bearer ", StringComparison.OrdinalIgnoreCase)) return Task.FromResult(AuthenticateResult.NoResult());
        var token = auth["Bearer ".Length..].Trim();
        var machine = token.Length == 0 ? null : TokenHash.Match(hub.CurrentValue.Machines, token);
        if (machine is null) return Task.FromResult(AuthenticateResult.Fail("unknown token"));
        var (id, error) = ApiAuth.ResolveMachine(machine, Request);
        if (id is null) return Task.FromResult(AuthenticateResult.Fail(error!));
        var identity = new ClaimsIdentity(new[] { new Claim(ClaimTypes.Name, id), new Claim(ApiAuth.MachineClaim, id) }, Scheme.Name);
        return Task.FromResult(AuthenticateResult.Success(new AuthenticationTicket(new ClaimsPrincipal(identity), Scheme.Name)));
    }
}
