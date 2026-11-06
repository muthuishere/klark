using System.Security.Claims;
using System.Text;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;

namespace KlarkHub.Services;

/// <summary>One agent on a linked machine, as GET /api/agents lists it (ADR 0009).</summary>
public sealed record AgentEntry(
    [property: JsonPropertyName("machine")] string Machine,
    [property: JsonPropertyName("session")] string Session,
    [property: JsonPropertyName("agent")] string? Agent,
    [property: JsonPropertyName("name")] string? Name,
    [property: JsonPropertyName("pane_id")] string PaneId,
    [property: JsonPropertyName("status")] string? Status,
    [property: JsonPropertyName("address")] string Address);

/// <summary>The outcome of a send: HTTP status, the message id when one was recorded, and the error text.</summary>
public sealed record SendOutcome(int Status, string? Id, string? Error);

/// <summary>
/// Agent-to-agent messaging across machines (ADR 0009). Shared by the /api
/// endpoints and the /messages page, so both apply the same rules.
/// Callers are users (cookie or JWT, checked with HubAccess) or machines
/// (machine token: one hub is one trust domain, so they reach every machine).
/// </summary>
public sealed class Messaging
{
    public const int MaxTextBytes = 16 * 1024;
    private static readonly TimeSpan Timeout = TimeSpan.FromSeconds(10);

    private readonly MachineRegistry _registry;
    private readonly HubStore _store;
    private readonly HubAccess _access;
    private readonly ILogger<Messaging> _log;

    public Messaging(MachineRegistry registry, HubStore store, HubAccess access, ILogger<Messaging> log)
    {
        _registry = registry;
        _store = store;
        _access = access;
        _log = log;
    }

    public bool CanSee(ClaimsPrincipal caller, string machineId) =>
        ApiAuth.MachineOf(caller) is not null || _access.CanSee(caller, machineId);

    public bool CanType(ClaimsPrincipal caller, string machineId) =>
        ApiAuth.MachineOf(caller) is not null || _access.CanType(caller, machineId);

    /// <summary>Who really called, recorded as the message's sender.</summary>
    public static string SenderOf(ClaimsPrincipal caller) =>
        ApiAuth.MachineOf(caller) is { } m ? "machine:" + m : "user:" + caller.Identity?.Name;

    /// <summary>Ask every online protocol-2 machine the caller may see for its agents, concurrently.</summary>
    public async Task<List<AgentEntry>> ListAgentsAsync(ClaimsPrincipal caller)
    {
        var machines = _registry.Machines.Where(m => m.Protocol >= 2 && CanSee(caller, m.Id)).ToList();
        var lists = await Task.WhenAll(machines.Select(async m =>
        {
            try
            {
                return await CachedAgents(m);
            }
            catch (Exception e) when (e is TimeoutException or InvalidOperationException or NotSupportedException)
            {
                _log.LogInformation("list_agents on {Machine} failed: {Error}", m.Id, e.Message);
                return new List<AgentEntry>();
            }
        }));
        return lists.SelectMany(l => l).OrderBy(a => a.Address, StringComparer.OrdinalIgnoreCase).ToList();
    }

    // One shared answer per machine for ~2 s, so many API clients do not each ask every machine.
    private static readonly TimeSpan CacheFor = TimeSpan.FromSeconds(2);
    private readonly System.Collections.Concurrent.ConcurrentDictionary<MachineConnection, (DateTime At, Task<List<AgentEntry>> List)> _agentCache = new();

    private Task<List<AgentEntry>> CachedAgents(MachineConnection m)
    {
        var now = DateTime.UtcNow;
        foreach (var k in _agentCache.Keys) if (!_registry.Machines.Contains(k)) _agentCache.TryRemove(k, out _);
        var entry = _agentCache.AddOrUpdate(m,
            _ => (now, Fetch(m)),
            (_, old) => now - old.At < CacheFor && !old.List.IsFaulted ? old : (now, Fetch(m)));
        return entry.List;
    }

    private async Task<List<AgentEntry>> Fetch(MachineConnection m)
    {
        var agents = await m.ListAgentsAsync(Timeout);
        return agents.OfType<JsonObject>().Select(a => ToEntry(m.Id, a)).OfType<AgentEntry>().ToList();
    }

    private static AgentEntry? ToEntry(string machineId, JsonObject a)
    {
        static string? Str(JsonNode? n) => n is JsonValue v && v.TryGetValue<string>(out var s) ? s : n?.ToJsonString();
        var session = Str(a["session"]) ?? "default";
        var paneId = Str(a["pane_id"]);
        if (paneId is null) return null;
        var name = Str(a["name"]);
        var who = string.IsNullOrEmpty(name) ? paneId : name;
        return new AgentEntry(machineId, session, Str(a["agent"]), name, paneId, Str(a["status"]), $"{machineId}/{session}/{who}");
    }

    /// <summary>Deliver one message to `machine/session/agent`, and record it.</summary>
    public async Task<SendOutcome> SendAsync(ClaimsPrincipal caller, string? to, string? text, string? from)
    {
        var parts = (to ?? "").Split('/');
        if (parts.Length != 3 || parts.Any(p => p.Trim().Length == 0))
            return new SendOutcome(400, null, "to must be machine/session/agent");
        if (string.IsNullOrEmpty(text)) return new SendOutcome(400, null, "text is empty");
        if (Encoding.UTF8.GetByteCount(text) > MaxTextBytes)
            return new SendOutcome(413, null, $"text is longer than {MaxTextBytes} bytes");
        var (machineName, session, agent) = (parts[0].Trim(), parts[1].Trim(), parts[2].Trim());

        var machine = _registry.Get(machineName)
            ?? _registry.Machines.FirstOrDefault(m => string.Equals(m.Id, machineName, StringComparison.OrdinalIgnoreCase));
        var machineId = machine?.Id ?? machineName;
        if (!CanSee(caller, machineId) || !CanType(caller, machineId))
            return new SendOutcome(403, null, $"you may not send to {machineId}");

        if (string.IsNullOrWhiteSpace(from))
            from = ApiAuth.MachineOf(caller) ?? "user:" + caller.Identity?.Name;
        var address = $"{machineId}/{session}/{agent}";
        var id = Guid.NewGuid().ToString("N");

        SendOutcome outcome;
        if (machine is null)
        {
            outcome = new SendOutcome(404, id, $"machine {machineName} is not linked to this hub");
        }
        else
        {
            try
            {
                await machine.DeliverAsync(session, agent, from, text, Timeout);
                outcome = new SendOutcome(200, id, null);
            }
            catch (NotSupportedException e) { outcome = new SendOutcome(409, id, e.Message); }
            catch (TimeoutException) { outcome = new SendOutcome(504, id, $"{machineId} did not answer in time"); }
            catch (InvalidOperationException e) { outcome = new SendOutcome(502, id, e.Message); }
        }
        _store.RecordMessage(new HubMessage(id, DateTimeOffset.UtcNow, from, address, text, SenderOf(caller),
            outcome.Status == 200, outcome.Error));
        return outcome;
    }

    /// <summary>Recent messages whose target or source machine the caller may see (or that the caller sent).</summary>
    public async Task<List<HubMessage>> RecentAsync(ClaimsPrincipal caller, int limit)
    {
        limit = Math.Clamp(limit, 1, 500);
        var all = await _store.RecentMessagesAsync(ApiAuth.MachineOf(caller) is null ? 5000 : limit);
        var me = SenderOf(caller);
        bool Visible(HubMessage m) =>
            m.Sender == me || CanSee(caller, MachineOf(m.To)) ||
            (!m.From.StartsWith("user:", StringComparison.Ordinal) && CanSee(caller, MachineOf(m.From)));
        return all.Where(Visible).Take(limit).ToList();
    }

    private static string MachineOf(string address) => address.Split('/', 2)[0];
}
