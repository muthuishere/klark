using System.DirectoryServices.Protocols;
using System.Net;
using System.Text;
using Microsoft.AspNetCore.Identity;
using Microsoft.Extensions.Options;

namespace KlarkHub.Services;

/// <summary>Who signed in, as the directory describes them.</summary>
public sealed record DirectoryUser(string Name, string DisplayName, IReadOnlyList<string> Groups);

/// <summary>Checks a name and password. Picked by Hub:Auth:Mode.</summary>
public interface IUserDirectory
{
    Task<DirectoryUser?> AuthenticateAsync(string username, string password, CancellationToken ct = default);
}

/// <summary>Applies Hub:Auth:AllowedGroups on top of whichever directory is configured.</summary>
public sealed class SignInService
{
    private readonly IServiceProvider _services;
    private readonly IOptionsMonitor<HubOptions> _options;
    private readonly ILogger<SignInService> _log;

    public SignInService(IServiceProvider services, IOptionsMonitor<HubOptions> options, ILogger<SignInService> log)
    {
        _services = services;
        _options = options;
        _log = log;
    }

    public async Task<DirectoryUser?> SignInAsync(string username, string password, CancellationToken ct = default)
    {
        if (string.IsNullOrWhiteSpace(username) || string.IsNullOrEmpty(password)) return null;
        var auth = _options.CurrentValue.Auth;
        IUserDirectory directory = auth.Mode.ToLowerInvariant() switch
        {
            "ldap" => _services.GetRequiredService<LdapDirectory>(),
            "local" => _services.GetRequiredService<LocalDirectory>(),
            _ => _services.GetRequiredService<MockLdapDirectory>(),
        };
        DirectoryUser? user;
        try
        {
            user = await directory.AuthenticateAsync(username.Trim(), password, ct);
        }
        catch (Exception e) when (e is LdapException or DirectoryOperationException)
        {
            _log.LogWarning(e, "directory error signing in {User}", username);
            return null;
        }
        if (user is null) return null;
        if (auth.AllowedGroups.Count > 0 &&
            !user.Groups.Any(g => auth.AllowedGroups.Contains(g, StringComparer.OrdinalIgnoreCase)))
        {
            _log.LogInformation("{User} is not in an allowed group", user.Name);
            return null;
        }
        return user;
    }
}

/// <summary>
/// A pretend LDAP directory held in memory, for development and demos. It
/// behaves like a bind against ou=users with group membership, without a
/// server. With no Hub:Auth:MockUsers configured it seeds:
///   admin/admin (klark-admins, klark-users), alice/alice and bob/bob (klark-users),
///   viewer/viewer (klark-users, klark-readonly: can watch, cannot type).
/// </summary>
public sealed class MockLdapDirectory : IUserDirectory
{
    private static readonly MockLdapEntry[] Seed =
    {
        new() { Uid = "admin", Password = "admin", DisplayName = "Administrator", Groups = new() { "klark-admins", "klark-users" } },
        new() { Uid = "alice", Password = "alice", DisplayName = "Alice", Groups = new() { "klark-users" } },
        new() { Uid = "bob", Password = "bob", DisplayName = "Bob", Groups = new() { "klark-users" } },
        new() { Uid = "viewer", Password = "viewer", DisplayName = "Viewer (read-only)", Groups = new() { "klark-users", "klark-readonly" } },
    };

    private readonly IOptionsMonitor<HubOptions> _options;

    public MockLdapDirectory(IOptionsMonitor<HubOptions> options, ILogger<MockLdapDirectory> log)
    {
        _options = options;
        if (options.CurrentValue.Auth.MockUsers.Count == 0)
        {
            log.LogWarning("Mock LDAP is using its seeded demo users (admin/admin, alice/alice, bob/bob, viewer/viewer). Do not expose this hub.");
        }
    }

    public Task<DirectoryUser?> AuthenticateAsync(string username, string password, CancellationToken ct = default)
    {
        var entries = _options.CurrentValue.Auth.MockUsers is { Count: > 0 } configured ? configured : Seed.ToList();
        var entry = entries.FirstOrDefault(e => string.Equals(e.Uid, username, StringComparison.OrdinalIgnoreCase));
        var ok = entry is not null && System.Security.Cryptography.CryptographicOperations.FixedTimeEquals(
            Encoding.UTF8.GetBytes(entry.Password), Encoding.UTF8.GetBytes(password));
        return Task.FromResult(ok
            ? new DirectoryUser(entry!.Uid, string.IsNullOrEmpty(entry.DisplayName) ? entry.Uid : entry.DisplayName, entry.Groups)
            : null);
    }
}

/// <summary>A real LDAP server: simple bind as the user, then read their groups.</summary>
public sealed class LdapDirectory : IUserDirectory
{
    private readonly IOptionsMonitor<HubOptions> _options;

    public LdapDirectory(IOptionsMonitor<HubOptions> options) => _options = options;

    public Task<DirectoryUser?> AuthenticateAsync(string username, string password, CancellationToken ct = default) =>
        Task.Run(() => Authenticate(username, password), ct);

    private DirectoryUser? Authenticate(string username, string password)
    {
        var o = _options.CurrentValue.Auth.Ldap;
        var dn = string.Format(o.UserDnTemplate, EscapeDnValue(username));
        using var conn = new LdapConnection(new LdapDirectoryIdentifier(o.Host, o.Port))
        {
            AuthType = AuthType.Basic,
            Timeout = TimeSpan.FromSeconds(10),
        };
        conn.SessionOptions.ProtocolVersion = 3;
        try
        {
            conn.Bind(new NetworkCredential(dn, password));
        }
        catch (LdapException e) when (e.ErrorCode == 49) // invalidCredentials
        {
            return null;
        }

        var displayName = username;
        var who = (SearchResponse)conn.SendRequest(new SearchRequest(dn, "(objectClass=*)", SearchScope.Base, "cn", "displayName"));
        if (who.Entries.Count > 0)
        {
            var e = who.Entries[0];
            displayName = First(e, "displayName") ?? First(e, "cn") ?? username;
        }

        var filter = $"(|(member={EscapeFilter(dn)})(uniqueMember={EscapeFilter(dn)})(memberUid={EscapeFilter(username)}))";
        var groups = (SearchResponse)conn.SendRequest(new SearchRequest(o.GroupSearchBase, filter, SearchScope.Subtree, "cn"));
        var names = groups.Entries.Cast<SearchResultEntry>().Select(g => First(g, "cn")).OfType<string>().ToList();
        return new DirectoryUser(username, displayName, names);
    }

    private static string? First(SearchResultEntry e, string attr) =>
        e.Attributes.Contains(attr) && e.Attributes[attr].Count > 0 ? e.Attributes[attr][0]?.ToString() : null;

    private static string EscapeDnValue(string v)
    {
        var sb = new StringBuilder();
        foreach (var c in v)
        {
            if (",+\"\\<>;=#".IndexOf(c) >= 0) sb.Append('\\');
            sb.Append(c);
        }
        return sb.ToString();
    }

    private static string EscapeFilter(string v)
    {
        var sb = new StringBuilder();
        foreach (var c in v)
        {
            sb.Append(c switch
            {
                '*' => "\\2a",
                '(' => "\\28",
                ')' => "\\29",
                '\\' => "\\5c",
                '\0' => "\\00",
                _ => c.ToString(),
            });
        }
        return sb.ToString();
    }
}

/// <summary>Users listed in Hub:Users with PasswordHasher hashes.</summary>
public sealed class LocalDirectory : IUserDirectory
{
    private readonly IOptionsMonitor<HubOptions> _options;

    public LocalDirectory(IOptionsMonitor<HubOptions> options) => _options = options;

    public Task<DirectoryUser?> AuthenticateAsync(string username, string password, CancellationToken ct = default)
    {
        var user = _options.CurrentValue.Users.FirstOrDefault(u => string.Equals(u.Name, username, StringComparison.OrdinalIgnoreCase));
        var ok = user is not null && !string.IsNullOrEmpty(user.PasswordHash) &&
                 new PasswordHasher<string>().VerifyHashedPassword("", user.PasswordHash, password) != PasswordVerificationResult.Failed;
        return Task.FromResult(ok ? new DirectoryUser(user!.Name, user.Name, Array.Empty<string>()) : null);
    }
}
