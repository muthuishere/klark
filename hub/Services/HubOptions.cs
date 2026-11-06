using System.Security.Cryptography;
using System.Text;

namespace KlarkHub.Services;

/// <summary>Bound from the "Hub" configuration section.</summary>
public sealed class HubOptions
{
    /// <summary>Where hub.db and machines/*.db live. Mount a volume here in Docker.</summary>
    public string DataDir { get; set; } = "data";

    /// <summary>Cap on each machine's SQLite file (ADR 0001, D7).</summary>
    public long MaxBytesPerMachine { get; set; } = 10 * 1024 * 1024;

    /// <summary>How people sign in: Local (Users below), MockLdap, or Ldap.</summary>
    public AuthOptions Auth { get; set; } = new();

    /// <summary>People who may sign in to the UI. Every signed-in user sees every connected machine.</summary>
    public List<HubUser> Users { get; set; } = new();

    /// <summary>User JWTs for the CLI and /api (ADR 0009).</summary>
    public JwtOptions Jwt { get; set; } = new();

    /// <summary>klark machines allowed to connect with `klark expose`.</summary>
    public List<HubMachine> Machines { get; set; } = new();
}

public sealed class JwtOptions
{
    /// <summary>HS256 signing key (KLARK_HUB__JWT__KEY), at least 32 bytes. Empty: a random key kept in DataDir/jwt.key.</summary>
    public string Key { get; set; } = "";

    /// <summary>Token lifetime.</summary>
    public double Hours { get; set; } = 12;
}

public sealed class HubUser
{
    public string Name { get; set; } = "";

    /// <summary>ASP.NET Core PasswordHasher output: `dotnet run -- hash-password &lt;pw&gt;`.</summary>
    public string PasswordHash { get; set; } = "";
}

public sealed class HubMachine
{
    public string Name { get; set; } = "";

    /// <summary>Hex SHA-256 of the machine token: `dotnet run -- hash-token &lt;token&gt;`.</summary>
    public string TokenSha256 { get; set; } = "";

    /// <summary>
    /// One token for many machines (load tests, fleets): each connection is
    /// named "{Name}:{X-Klark-Machine header}" instead of just Name.
    /// </summary>
    public bool Fleet { get; set; }

    /// <summary>
    /// Groups allowed to see this machine (STATUS S-05). Empty: everyone signed in.
    /// Members of Auth.AdminGroups always see every machine.
    /// </summary>
    public List<string> Groups { get; set; } = new();
}

public static class TokenHash
{
    public static string Sha256Hex(string token) =>
        Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(token))).ToLowerInvariant();

    /// <summary>The configured machine whose token matches, compared in constant time.</summary>
    public static HubMachine? Match(IEnumerable<HubMachine> machines, string token)
    {
        var presented = Encoding.ASCII.GetBytes(Sha256Hex(token));
        HubMachine? found = null;
        foreach (var m in machines)
        {
            var expected = Encoding.ASCII.GetBytes(m.TokenSha256.Trim().ToLowerInvariant());
            if (expected.Length == presented.Length && CryptographicOperations.FixedTimeEquals(expected, presented))
            {
                found = m;
            }
        }
        return found;
    }
}

public sealed class AuthOptions
{
    /// <summary>
    /// "Ldap" (default: your LDAP / Active Directory checks every password),
    /// "MockLdap" (built-in pretend directory, development only) or "Local" (hashes in Hub:Users).
    /// </summary>
    public string Mode { get; set; } = "Ldap";

    /// <summary>If not empty, only members of one of these groups may sign in.</summary>
    public List<string> AllowedGroups { get; set; } = new();

    /// <summary>Members see every machine, whatever its Groups say.</summary>
    public List<string> AdminGroups { get; set; } = new() { "klark-admins" };

    /// <summary>Members can watch panes but never type into them (STATUS S-06).</summary>
    public List<string> ReadOnlyGroups { get; set; } = new() { "klark-readonly" };

    public LdapOptions Ldap { get; set; } = new();

    /// <summary>Entries of the mock directory. Empty means the seeded demo users.</summary>
    public List<MockLdapEntry> MockUsers { get; set; } = new();
}

public sealed class LdapOptions
{
    public string Host { get; set; } = "localhost";

    /// <summary>0 = pick from the scheme: 636 with UseSsl, else 389.</summary>
    public int Port { get; set; }

    /// <summary>LDAPS (TLS from the first byte, usually port 636).</summary>
    public bool UseSsl { get; set; }

    /// <summary>Upgrade a plain connection with StartTLS before binding.</summary>
    public bool StartTls { get; set; }

    /// <summary>Accept any server certificate (self-signed test servers only).</summary>
    public bool SkipCertificateCheck { get; set; }

    /// <summary>
    /// Direct bind: the DN (or UPN) to bind as, with {0} = the escaped user name.
    /// Examples: "uid={0},ou=people,dc=example,dc=com" (OpenLDAP) or "{0}@corp.example.com" (Active Directory).
    /// Ignored when UserSearchBase is set.
    /// </summary>
    public string UserDnTemplate { get; set; } = "cn={0},ou=users,dc=klark,dc=local";

    /// <summary>
    /// Search-then-bind: when set, the hub finds the user under this base with UserFilter
    /// (bound as BindDn/BindPassword, or anonymously), then binds as the DN it found.
    /// </summary>
    public string UserSearchBase { get; set; } = "";

    /// <summary>{0} = the escaped user name. "(uid={0})" for OpenLDAP, "(sAMAccountName={0})" for AD.</summary>
    public string UserFilter { get; set; } = "(|(uid={0})(sAMAccountName={0})(cn={0}))";

    /// <summary>Service account for search-then-bind. Set the password through the environment, not a file.</summary>
    public string BindDn { get; set; } = "";
    public string BindPassword { get; set; } = "";

    /// <summary>Where groups live; members are matched by DN (member, uniqueMember) or name (memberUid). Empty = skip the search.</summary>
    public string GroupSearchBase { get; set; } = "dc=klark,dc=local";

    /// <summary>Also read the user's memberOf attribute (Active Directory, OpenLDAP memberof overlay).</summary>
    public bool UseMemberOf { get; set; } = true;

    public int TimeoutSeconds { get; set; } = 10;
}

public sealed class MockLdapEntry
{
    public string Uid { get; set; } = "";

    /// <summary>Plain text on purpose: this is a mock directory for development.</summary>
    public string Password { get; set; } = "";
    public string DisplayName { get; set; } = "";
    public List<string> Groups { get; set; } = new();
}
