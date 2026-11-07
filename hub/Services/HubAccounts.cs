using System.Text.Json;
using Microsoft.AspNetCore.Identity;
using Microsoft.Data.Sqlite;
using Microsoft.Extensions.Options;

namespace KlarkHub.Services;

/// <summary>A user of the mock directory, as stored in hub.db (no password).</summary>
public sealed record MockUser(string Uid, string DisplayName, IReadOnlyList<string> Groups);

/// <summary>
/// Accounts and sign-in settings kept in hub.db (ADR 0009, "Sign-in modes"):
///  - the built-in admin (always present; password hash only),
///  - the MockLdap directory's users (hashed passwords, editable in /admin/users),
///  - the directory mode and LDAP settings chosen in /admin/directory.
/// Configuration stays the default whenever nothing is stored. Passwords are never logged.
/// </summary>
public sealed class HubAccounts
{
    public const string AdminName = "admin";
    public const string DefaultAdminPassword = "adminadmin";
    /// <summary>Role every built-in admin session carries, whatever Auth:AdminGroups says.</summary>
    public const string AdminRole = "klark-builtin-admin";

    private readonly IOptionsMonitor<HubOptions> _options;
    private readonly ILogger<HubAccounts> _log;
    private readonly PasswordHasher<string> _hasher = new();
    private readonly object _gate = new();
    private bool _ready;

    public HubAccounts(IOptionsMonitor<HubOptions> options, ILogger<HubAccounts> log)
    {
        _options = options;
        _log = log;
    }

    private string DbPath => Path.Combine(_options.CurrentValue.DataDir, "hub.db");

    // ---- schema and first-start seed ------------------------------------------

    /// <summary>Creates the tables and seeds the admin and mock users once. Safe to call repeatedly.</summary>
    public void EnsureReady()
    {
        if (_ready) return;
        lock (_gate)
        {
            if (_ready) return;
            Directory.CreateDirectory(_options.CurrentValue.DataDir);
            using var db = Open();
            Exec(db, """
                PRAGMA journal_mode = WAL;
                CREATE TABLE IF NOT EXISTS hub_admin (
                  name TEXT PRIMARY KEY, hash TEXT NOT NULL, is_default INTEGER NOT NULL);
                CREATE TABLE IF NOT EXISTS mock_users (
                  uid TEXT PRIMARY KEY COLLATE NOCASE, display_name TEXT NOT NULL, groups TEXT NOT NULL, hash TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS hub_machines (
                  name TEXT PRIMARY KEY COLLATE NOCASE, token_sha256 TEXT NOT NULL, created INTEGER NOT NULL,
                  approved_by TEXT NOT NULL DEFAULT '');
                """);
            if (Scalar(db, "SELECT COUNT(*) FROM hub_admin") == 0)
            {
                var configured = _options.CurrentValue.Admin.Password;
                var pw = string.IsNullOrEmpty(configured) ? DefaultAdminPassword : configured;
                Run(db, "INSERT INTO hub_admin (name, hash, is_default) VALUES ($n, $h, $d)",
                    ("$n", AdminName), ("$h", _hasher.HashPassword(AdminName, pw)), ("$d", pw == DefaultAdminPassword ? 1 : 0));
                _log.LogInformation("created the built-in admin account{Default}",
                    pw == DefaultAdminPassword ? " with the default password; change it at /account/password" : "");
            }
            if (Scalar(db, "SELECT COUNT(*) FROM mock_users") == 0)
            {
                var seed = _options.CurrentValue.Auth.MockUsers is { Count: > 0 } cfg
                    ? cfg.Select(e => (e.Uid, e.DisplayName, (IReadOnlyList<string>)e.Groups, e.Password))
                    : new[]
                    {
                        ("user1", "User One", (IReadOnlyList<string>)new[] { "klark-users" }, "user1pass"),
                        ("user2", "User Two", (IReadOnlyList<string>)new[] { "klark-users" }, "user2pass"),
                    };
                foreach (var (uid, display, groups, pw) in seed)
                {
                    if (string.IsNullOrWhiteSpace(uid) || string.IsNullOrEmpty(pw)) continue;
                    Run(db, "INSERT OR IGNORE INTO mock_users (uid, display_name, groups, hash) VALUES ($u, $d, $g, $h)",
                        ("$u", uid.Trim()), ("$d", string.IsNullOrWhiteSpace(display) ? uid.Trim() : display.Trim()),
                        ("$g", JsonSerializer.Serialize(groups)), ("$h", _hasher.HashPassword(uid, pw)));
                }
                _log.LogInformation("seeded the mock directory's users");
            }
            _ready = true;
        }
    }

    // ---- built-in admin ---------------------------------------------------------

    public bool IsAdminName(string username) => string.Equals(username.Trim(), AdminName, StringComparison.OrdinalIgnoreCase);

    public bool VerifyAdmin(string password)
    {
        EnsureReady();
        using var db = Open();
        var hash = Text(db, "SELECT hash FROM hub_admin WHERE name = $n", ("$n", AdminName));
        return hash is not null && Verify(hash, password);
    }

    /// <summary>True while the built-in admin still has the default password (drives the banner).</summary>
    public bool AdminPasswordIsDefault()
    {
        EnsureReady();
        using var db = Open();
        return Scalar(db, "SELECT is_default FROM hub_admin WHERE name = $n", ("$n", AdminName)) != 0;
    }

    public DirectoryUser AdminUser() =>
        new(AdminName, "Administrator",
            new[] { AdminRole }.Concat(_options.CurrentValue.Auth.AdminGroups).Distinct(StringComparer.OrdinalIgnoreCase).ToList(),
            DirectoryUser.BuiltIn);

    // ---- mock directory users ----------------------------------------------------

    public List<MockUser> MockUsers()
    {
        EnsureReady();
        using var db = Open();
        using var cmd = db.CreateCommand();
        cmd.CommandText = "SELECT uid, display_name, groups FROM mock_users ORDER BY uid COLLATE NOCASE";
        using var r = cmd.ExecuteReader();
        var list = new List<MockUser>();
        while (r.Read()) list.Add(new MockUser(r.GetString(0), r.GetString(1), Groups(r.GetString(2))));
        return list;
    }

    public DirectoryUser? VerifyMockUser(string username, string password)
    {
        EnsureReady();
        using var db = Open();
        using var cmd = db.CreateCommand();
        cmd.CommandText = "SELECT uid, display_name, groups, hash FROM mock_users WHERE uid = $u";
        cmd.Parameters.AddWithValue("$u", username.Trim());
        using var r = cmd.ExecuteReader();
        if (!r.Read() || !Verify(r.GetString(3), password)) return null;
        return new DirectoryUser(r.GetString(0), r.GetString(1), Groups(r.GetString(2)), DirectoryUser.Mock);
    }

    /// <summary>Adds a mock user; returns an error to show, or null.</summary>
    public string? AddMockUser(string uid, string? displayName, IEnumerable<string> groups, string password)
    {
        uid = (uid ?? "").Trim();
        if (uid.Length == 0 || uid.Length > 64 || uid.Any(c => char.IsWhiteSpace(c) || char.IsControl(c)))
            return "Name is required, without spaces, at most 64 characters.";
        if (IsAdminName(uid)) return "That name is reserved for the built-in admin.";
        if (PasswordProblem(password) is { } p) return p;
        EnsureReady();
        using var db = Open();
        var changed = Run(db, "INSERT OR IGNORE INTO mock_users (uid, display_name, groups, hash) VALUES ($u, $d, $g, $h)",
            ("$u", uid), ("$d", string.IsNullOrWhiteSpace(displayName) ? uid : displayName.Trim()),
            ("$g", JsonSerializer.Serialize(CleanGroups(groups))), ("$h", _hasher.HashPassword(uid, password)));
        if (changed == 0) return $"A user named {uid} already exists.";
        _log.LogInformation("mock directory: added {User}", uid);
        return null;
    }

    public string? ResetMockPassword(string uid, string password)
    {
        if (PasswordProblem(password) is { } p) return p;
        EnsureReady();
        using var db = Open();
        var changed = Run(db, "UPDATE mock_users SET hash = $h WHERE uid = $u", ("$u", uid.Trim()), ("$h", _hasher.HashPassword(uid, password)));
        if (changed == 0) return "No such user.";
        _log.LogInformation("mock directory: password reset for {User}", uid);
        return null;
    }

    public bool DeleteMockUser(string uid)
    {
        EnsureReady();
        using var db = Open();
        var ok = Run(db, "DELETE FROM mock_users WHERE uid = $u", ("$u", uid.Trim())) > 0;
        if (ok) _log.LogInformation("mock directory: deleted {User}", uid);
        return ok;
    }

    // ---- own password -----------------------------------------------------------

    /// <summary>
    /// Changes the password of the built-in admin or of a mock-directory user, after
    /// checking the current one. Returns an error to show, or null on success.
    /// </summary>
    public string? ChangeOwnPassword(string username, string current, string next)
    {
        if (PasswordProblem(next) is { } p) return p;
        EnsureReady();
        using var db = Open();
        if (IsAdminName(username))
        {
            if (!VerifyAdmin(current)) return "The current password is wrong.";
            Run(db, "UPDATE hub_admin SET hash = $h, is_default = $d WHERE name = $n",
                ("$n", AdminName), ("$h", _hasher.HashPassword(AdminName, next)), ("$d", next == DefaultAdminPassword ? 1 : 0));
            _log.LogInformation("built-in admin changed its password");
            return null;
        }
        if (VerifyMockUser(username, current) is null)
        {
            return MockUsers().Any(u => string.Equals(u.Uid, username, StringComparison.OrdinalIgnoreCase))
                ? "The current password is wrong."
                : "Your password is managed by your directory, not by this hub.";
        }
        Run(db, "UPDATE mock_users SET hash = $h WHERE uid = $u", ("$u", username.Trim()), ("$h", _hasher.HashPassword(username, next)));
        _log.LogInformation("mock directory: {User} changed their password", username);
        return null;
    }

    public static string? PasswordProblem(string? password) =>
        string.IsNullOrEmpty(password) || password.Length < 8 ? "Use a password of at least 8 characters." : null;

    // ---- directory mode and LDAP settings ------------------------------------------

    /// <summary>The mode in force now: stored in hub.db, else configuration.</summary>
    public string EffectiveMode()
    {
        EnsureReady();
        using var db = Open();
        var stored = Text(db, "SELECT value FROM settings WHERE key = 'auth.mode'");
        return string.IsNullOrWhiteSpace(stored) ? _options.CurrentValue.Auth.Mode : stored;
    }

    /// <summary>True when the LDAP settings in force come from hub.db rather than configuration.</summary>
    public bool LdapIsStored()
    {
        EnsureReady();
        using var db = Open();
        return Text(db, "SELECT value FROM settings WHERE key = 'auth.ldap'") is not null;
    }

    /// <summary>The LDAP settings in force now: stored in hub.db, else configuration.</summary>
    public LdapOptions EffectiveLdap()
    {
        EnsureReady();
        using var db = Open();
        var stored = Text(db, "SELECT value FROM settings WHERE key = 'auth.ldap'");
        return stored is null ? _options.CurrentValue.Auth.Ldap : JsonSerializer.Deserialize<LdapOptions>(stored) ?? _options.CurrentValue.Auth.Ldap;
    }

    public static readonly string[] Modes = { "MockLdap", "Ldap", "Local" };

    /// <summary>Stores the sign-in mode; takes effect on the next sign-in.</summary>
    public string? SaveMode(string mode)
    {
        var known = Modes.FirstOrDefault(m => m.Equals(mode, StringComparison.OrdinalIgnoreCase));
        if (known is null) return "Pick MockLdap, Ldap or Local.";
        EnsureReady();
        using var db = Open();
        Run(db, "INSERT INTO settings (key, value) VALUES ('auth.mode', $v) ON CONFLICT(key) DO UPDATE SET value = $v", ("$v", known));
        _log.LogInformation("sign-in mode set to {Mode}", known);
        return null;
    }

    /// <summary>Stores LDAP settings. An empty BindPassword keeps the one in force.</summary>
    public string? SaveLdap(LdapOptions ldap)
    {
        if (string.IsNullOrWhiteSpace(ldap.Host)) return "LDAP needs a host.";
        if (ldap.Port is < 0 or > 65535) return "Port must be 0-65535.";
        EnsureReady();
        if (string.IsNullOrEmpty(ldap.BindPassword)) ldap.BindPassword = EffectiveLdap().BindPassword;
        using var db = Open();
        Run(db, "INSERT INTO settings (key, value) VALUES ('auth.ldap', $v) ON CONFLICT(key) DO UPDATE SET value = $v",
            ("$v", JsonSerializer.Serialize(ldap)));
        _log.LogInformation("ldap settings saved: {Host}", ldap.Host);
        return null;
    }

    // ---- machine tokens created in /admin/machines ---------------------------------

    private volatile List<HubMachine>? _dbMachines;

    /// <summary>Machines created in the UI (only the token's SHA-256 is stored).</summary>
    public IReadOnlyList<HubMachine> DbMachines()
    {
        if (_dbMachines is { } cached) return cached;
        EnsureReady();
        using var db = Open();
        using var cmd = db.CreateCommand();
        cmd.CommandText = "SELECT name, token_sha256 FROM hub_machines ORDER BY name COLLATE NOCASE";
        using var r = cmd.ExecuteReader();
        var list = new List<HubMachine>();
        while (r.Read()) list.Add(new HubMachine { Name = r.GetString(0), TokenSha256 = r.GetString(1) });
        return _dbMachines = list;
    }

    /// <summary>Configured (Hub:Machines) and UI-created machines together.</summary>
    public IEnumerable<HubMachine> AllMachines() => _options.CurrentValue.Machines.Concat(DbMachines());

    /// <summary>The machine whose token matches, from configuration or hub.db.</summary>
    public HubMachine? MatchToken(string token) => token.Length == 0 ? null : TokenHash.Match(AllMachines(), token);

    public sealed record MachineRow(string Name, DateTimeOffset Created, string ApprovedBy);

    public List<MachineRow> MachineRows()
    {
        EnsureReady();
        using var db = Open();
        using var cmd = db.CreateCommand();
        cmd.CommandText = "SELECT name, created, approved_by FROM hub_machines ORDER BY name COLLATE NOCASE";
        using var r = cmd.ExecuteReader();
        var list = new List<MachineRow>();
        while (r.Read()) list.Add(new MachineRow(r.GetString(0), DateTimeOffset.FromUnixTimeMilliseconds(r.GetInt64(1)), r.GetString(2)));
        return list;
    }

    public static bool ValidMachineName(string name) =>
        name.Length is > 0 and <= 64 && name.All(c => char.IsAsciiLetterOrDigit(c) || c is '-' or '_' or '.');

    /// <summary>
    /// Creates (or, for its owner or an admin, re-keys) a machine and returns its new
    /// token, which is never stored: only its SHA-256 is. Or an error.
    /// </summary>
    public (string? Token, string? Error) CreateMachine(string name, string approvedBy, bool approverIsAdmin = true)
    {
        name = (name ?? "").Trim();
        if (!ValidMachineName(name)) return (null, "Use letters, digits, '-', '_' or '.', at most 64 characters.");
        if (_options.CurrentValue.Machines.Any(m => string.Equals(m.Name, name, StringComparison.OrdinalIgnoreCase)))
            return (null, $"{name} is already configured in Hub:Machines.");
        var existing = MachineRows().FirstOrDefault(m => string.Equals(m.Name, name, StringComparison.OrdinalIgnoreCase));
        if (existing is not null && !approverIsAdmin && !string.Equals(existing.ApprovedBy, approvedBy, StringComparison.OrdinalIgnoreCase))
            return (null, $"A machine named {name} already exists and belongs to someone else.");
        var token = "km_" + Convert.ToHexString(System.Security.Cryptography.RandomNumberGenerator.GetBytes(32)).ToLowerInvariant();
        using var db = Open();
        Run(db, """
            INSERT INTO hub_machines (name, token_sha256, created, approved_by) VALUES ($n, $h, $t, $a)
            ON CONFLICT(name) DO UPDATE SET token_sha256 = $h, created = $t, approved_by = $a
            """,
            ("$n", name), ("$h", TokenHash.Sha256Hex(token)), ("$t", DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()), ("$a", approvedBy));
        _dbMachines = null;
        _log.LogInformation("machine {Machine} {Action} by {User}", name, existing is null ? "created" : "re-keyed", approvedBy);
        return (token, null);
    }

    public bool RevokeMachine(string name)
    {
        EnsureReady();
        using var db = Open();
        var ok = Run(db, "DELETE FROM hub_machines WHERE name = $n", ("$n", name.Trim())) > 0;
        _dbMachines = null;
        if (ok) _log.LogInformation("machine {Machine} revoked", name);
        return ok;
    }

    // ---- helpers ------------------------------------------------------------------

    private bool Verify(string hash, string password) =>
        !string.IsNullOrEmpty(password) && _hasher.VerifyHashedPassword("", hash, password) != PasswordVerificationResult.Failed;

    private static List<string> CleanGroups(IEnumerable<string> groups) =>
        groups.Select(g => g.Trim()).Where(g => g.Length > 0).Distinct(StringComparer.OrdinalIgnoreCase).ToList();

    public static List<string> SplitGroups(string? text) =>
        CleanGroups((text ?? "").Split(new[] { ',', ' ', ';' }, StringSplitOptions.RemoveEmptyEntries));

    private static IReadOnlyList<string> Groups(string json) => JsonSerializer.Deserialize<List<string>>(json) ?? new List<string>();

    private SqliteConnection Open()
    {
        var db = new SqliteConnection(new SqliteConnectionStringBuilder { DataSource = DbPath, Pooling = false }.ToString());
        db.Open();
        return db;
    }

    private static void Exec(SqliteConnection db, string sql)
    {
        using var cmd = db.CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }

    private static int Run(SqliteConnection db, string sql, params (string Name, object Value)[] args)
    {
        using var cmd = db.CreateCommand();
        cmd.CommandText = sql;
        foreach (var (name, value) in args) cmd.Parameters.AddWithValue(name, value);
        return cmd.ExecuteNonQuery();
    }

    private static long Scalar(SqliteConnection db, string sql, params (string Name, object Value)[] args)
    {
        using var cmd = db.CreateCommand();
        cmd.CommandText = sql;
        foreach (var (name, value) in args) cmd.Parameters.AddWithValue(name, value);
        return Convert.ToInt64(cmd.ExecuteScalar() ?? 0L);
    }

    private static string? Text(SqliteConnection db, string sql, params (string Name, object Value)[] args)
    {
        using var cmd = db.CreateCommand();
        cmd.CommandText = sql;
        foreach (var (name, value) in args) cmd.Parameters.AddWithValue(name, value);
        return cmd.ExecuteScalar() as string;
    }
}
