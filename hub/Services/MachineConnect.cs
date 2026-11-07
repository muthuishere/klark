using System.Collections.Concurrent;
using System.Security.Cryptography;

namespace KlarkHub.Services;

/// <summary>
/// Device authorization for `klark hub connect` (ADR 0009, "Connecting a machine"):
/// the machine asks for a code, a signed-in person approves it at /connect, and
/// the machine's poll receives a fresh token exactly once. Pending requests live
/// in memory (one hub replica); only the approved token's SHA-256 reaches hub.db.
/// </summary>
public sealed class MachineConnect
{
    public static readonly TimeSpan Lifetime = TimeSpan.FromMinutes(10);
    public const int IntervalSeconds = 2;
    private const string CodeAlphabet = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // no 0/O, 1/I

    private readonly ConcurrentDictionary<string, Pending> _byPoll = new();
    private readonly HubAccounts _accounts;
    private readonly ILogger<MachineConnect> _log;

    public MachineConnect(HubAccounts accounts, ILogger<MachineConnect> log)
    {
        _accounts = accounts;
        _log = log;
    }

    /// <summary>For tests: the clock.</summary>
    public Func<DateTimeOffset> Now { get; set; } = () => DateTimeOffset.UtcNow;

    public sealed class Pending
    {
        public required string Code { get; init; }
        public required string PollId { get; init; }
        public required string Machine { get; init; }
        public required DateTimeOffset Expires { get; init; }
        public string Status { get; set; } = "pending";
        public string? Token { get; set; }
        public string? DecidedBy { get; set; }
    }

    public (Pending? Request, string? Error) Start(string? name)
    {
        name = (name ?? "").Trim();
        if (!HubAccounts.ValidMachineName(name)) return (null, "name: use letters, digits, '-', '_' or '.', at most 64 characters");
        Sweep();
        if (_byPoll.Count > 1000) return (null, "too many pending requests; try again later");
        var p = new Pending
        {
            Code = NewCode(),
            PollId = Convert.ToHexString(RandomNumberGenerator.GetBytes(32)).ToLowerInvariant(),
            Machine = name,
            Expires = Now() + Lifetime,
        };
        _byPoll[p.PollId] = p;
        _log.LogInformation("connect request {Code} for machine {Machine}", p.Code, name);
        return (p, null);
    }

    /// <summary>A pending, unexpired request by its code (what /connect shows).</summary>
    public Pending? Find(string? code)
    {
        var c = Normalize(code);
        if (c is null) return null;
        var p = _byPoll.Values.FirstOrDefault(x => x.Code == c);
        return p is not null && p.Expires > Now() ? p : null;
    }

    /// <summary>Approve or deny; on approval the machine is created and its token held for one poll.</summary>
    public string? Decide(string? code, bool approve, string user, bool isAdmin)
    {
        var p = Find(code);
        if (p is null) return "This code is unknown or has expired. Run klark hub connect again.";
        lock (p)
        {
            if (p.Status != "pending") return "This request was already decided.";
            p.DecidedBy = user;
            if (!approve)
            {
                p.Status = "denied";
                _log.LogInformation("connect request {Code} denied by {User}", p.Code, user);
                return null;
            }
            var (token, error) = _accounts.CreateMachine(p.Machine, user, isAdmin);
            if (token is null) return error;
            p.Token = token;
            p.Status = "approved";
            return null;
        }
    }

    /// <summary>pending | denied | expired | unknown, or approved with the token (once; the request is then consumed).</summary>
    public (string Status, string? Token, string? Machine) Poll(string? pollId)
    {
        if (string.IsNullOrEmpty(pollId) || !_byPoll.TryGetValue(pollId, out var p)) return ("unknown", null, null);
        lock (p)
        {
            if (p.Status == "approved")
            {
                _byPoll.TryRemove(pollId, out _);
                var token = p.Token;
                p.Token = null;
                return ("approved", token, p.Machine);
            }
            if (p.Expires <= Now())
            {
                _byPoll.TryRemove(pollId, out _);
                return ("expired", null, null);
            }
            if (p.Status == "denied") _byPoll.TryRemove(pollId, out _);
            return (p.Status, null, null);
        }
    }

    private void Sweep()
    {
        var now = Now();
        foreach (var (k, v) in _byPoll)
        {
            if (v.Expires <= now) _byPoll.TryRemove(k, out _);
        }
    }

    private static string NewCode()
    {
        var b = RandomNumberGenerator.GetBytes(8);
        var chars = b.Select(x => CodeAlphabet[x % CodeAlphabet.Length]).ToArray();
        return new string(chars, 0, 4) + "-" + new string(chars, 4, 4);
    }

    private static string? Normalize(string? code)
    {
        var c = new string((code ?? "").ToUpperInvariant().Where(char.IsAsciiLetterOrDigit).ToArray());
        return c.Length == 8 ? c[..4] + "-" + c[4..] : null;
    }
}
