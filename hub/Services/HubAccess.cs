using System.Security.Claims;
using Microsoft.Extensions.Options;

namespace KlarkHub.Services;

/// <summary>
/// Who may see and drive which machine. Every check runs on the server: Blazor
/// Server event handlers execute here, so hiding a button is never the only guard.
/// </summary>
public sealed class HubAccess
{
    private readonly IOptionsMonitor<HubOptions> _options;

    public HubAccess(IOptionsMonitor<HubOptions> options) => _options = options;

    public bool IsAdmin(ClaimsPrincipal user) =>
        user.Identity?.IsAuthenticated == true &&
        (user.IsInRole(HubAccounts.AdminRole) || InAny(user, _options.CurrentValue.Auth.AdminGroups));

    public bool IsReadOnly(ClaimsPrincipal user) =>
        !IsAdmin(user) && InAny(user, _options.CurrentValue.Auth.ReadOnlyGroups);

    /// <summary>The groups configured for a machine id (fleet members inherit their entry's groups).</summary>
    public IReadOnlyList<string> GroupsOf(string machineId)
    {
        var name = machineId.Split(':', 2)[0];
        return _options.CurrentValue.Machines
            .FirstOrDefault(m => string.Equals(m.Name, name, StringComparison.OrdinalIgnoreCase))?.Groups
            ?? (IReadOnlyList<string>)Array.Empty<string>();
    }

    public bool CanSee(ClaimsPrincipal user, string machineId)
    {
        if (user.Identity?.IsAuthenticated != true) return false;
        if (IsAdmin(user)) return true;
        var groups = GroupsOf(machineId);
        return groups.Count == 0 || InAny(user, groups);
    }

    public bool CanType(ClaimsPrincipal user, string machineId) =>
        CanSee(user, machineId) && !IsReadOnly(user);

    private static bool InAny(ClaimsPrincipal user, IEnumerable<string> groups) =>
        groups.Any(g => user.IsInRole(g));
}
