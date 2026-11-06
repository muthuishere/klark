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

    /// <summary>klark machines allowed to connect with `klark link`.</summary>
    public List<HubMachine> Machines { get; set; } = new();
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
    /// <summary>"MockLdap" (default: built-in pretend directory), "Ldap" (real server), or "Local".</summary>
    public string Mode { get; set; } = "MockLdap";

    /// <summary>If not empty, only members of one of these groups may sign in.</summary>
    public List<string> AllowedGroups { get; set; } = new();

    public LdapOptions Ldap { get; set; } = new();

    /// <summary>Entries of the mock directory. Empty means the seeded demo users.</summary>
    public List<MockLdapEntry> MockUsers { get; set; } = new();
}

public sealed class LdapOptions
{
    public string Host { get; set; } = "localhost";
    public int Port { get; set; } = 389;

    /// <summary>DN to bind as, with {0} = the escaped user name.</summary>
    public string UserDnTemplate { get; set; } = "cn={0},ou=users,dc=klark,dc=local";

    /// <summary>Where groups live; members are matched by DN (member) or name (memberUid).</summary>
    public string GroupSearchBase { get; set; } = "dc=klark,dc=local";
}

public sealed class MockLdapEntry
{
    public string Uid { get; set; } = "";

    /// <summary>Plain text on purpose: this is a mock directory for development.</summary>
    public string Password { get; set; } = "";
    public string DisplayName { get; set; } = "";
    public List<string> Groups { get; set; } = new();
}
