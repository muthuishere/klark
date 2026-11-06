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
