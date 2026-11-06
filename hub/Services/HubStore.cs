using System.Text.RegularExpressions;
using System.Threading.Channels;
using Microsoft.Data.Sqlite;
using Microsoft.Extensions.Options;

namespace KlarkHub.Services;

/// <summary>
/// SQLite persistence (ADR 0001, D7).
///
/// data/hub.db              — every machine that has connected, with last seen,
///                            and agent messages (ADR 0009).
/// data/machines/{id}.db    — one file per klark machine: latest tree, recent
///                            pane output, connection events. Capped at
///                            MaxBytesPerMachine; oldest output is pruned first.
///
/// All writes go through one bounded channel and are applied in batches by a
/// single background writer, so the WebSocket loops never wait on disk.
/// </summary>
public sealed partial class HubStore : BackgroundService
{
    private readonly HubOptions _options;
    private readonly ILogger<HubStore> _log;
    private readonly Channel<StoreItem> _queue = Channel.CreateBounded<StoreItem>(
        new BoundedChannelOptions(50_000) { FullMode = BoundedChannelFullMode.DropOldest, SingleReader = true });
    private readonly Dictionary<string, SqliteConnection> _machineDbs = new();
    private SqliteConnection? _hubDb;

    public HubStore(IOptions<HubOptions> options, ILogger<HubStore> log)
    {
        _options = options.Value;
        _log = log;
    }

    // ---- producers (called from WebSocket loops; never block) ---------------

    public void MachineConnected(string machineId, string host, string version) =>
        _queue.Writer.TryWrite(new StoreItem(machineId, ItemKind.Connected, host, version, null));

    public void MachineDisconnected(string machineId) =>
        _queue.Writer.TryWrite(new StoreItem(machineId, ItemKind.Disconnected, null, null, null));

    public void SaveTree(string machineId, string treeJson) =>
        _queue.Writer.TryWrite(new StoreItem(machineId, ItemKind.Tree, treeJson, null, null));

    public void AppendOutput(string machineId, string paneId, byte[] bytes) =>
        _queue.Writer.TryWrite(new StoreItem(machineId, ItemKind.Output, paneId, null, bytes));

    /// <summary>One agent message (ADR 0009), whatever its outcome.</summary>
    public void RecordMessage(HubMessage m) =>
        _queue.Writer.TryWrite(new StoreItem("", ItemKind.Message, null, null, null, m));

    // ---- readers -------------------------------------------------------------

    /// <summary>The newest messages, newest first.</summary>
    public async Task<List<HubMessage>> RecentMessagesAsync(int limit)
    {
        var hubDb = Path.Combine(_options.DataDir, "hub.db");
        if (!File.Exists(hubDb)) return new List<HubMessage>();
        await using var db = Open(hubDb);
        await using var cmd = db.CreateCommand();
        cmd.CommandText = "SELECT id, ts, from_addr, to_addr, text, sender, ok, error FROM messages ORDER BY ts DESC, rowid DESC LIMIT $limit";
        cmd.Parameters.AddWithValue("$limit", limit);
        var list = new List<HubMessage>();
        try
        {
            await using var r = await cmd.ExecuteReaderAsync();
            while (await r.ReadAsync())
            {
                list.Add(new HubMessage(r.GetString(0), DateTimeOffset.FromUnixTimeMilliseconds(r.GetInt64(1)), r.GetString(2),
                    r.GetString(3), r.GetString(4), r.GetString(5), r.GetInt64(6) != 0, r.IsDBNull(7) ? null : r.GetString(7)));
            }
        }
        catch (SqliteException e) when (e.Message.Contains("no such table")) { }
        return list;
    }

    /// <summary>Machines known to the hub, connected or not.</summary>
    public async Task<List<KnownMachine>> KnownMachinesAsync()
    {
        var hubDb = Path.Combine(_options.DataDir, "hub.db");
        if (!File.Exists(hubDb)) return new List<KnownMachine>();
        await using var db = Open(hubDb);
        await using var cmd = db.CreateCommand();
        cmd.CommandText = "SELECT id, host, version, first_seen, last_seen FROM machines ORDER BY id";
        var list = new List<KnownMachine>();
        await using var r = await cmd.ExecuteReaderAsync();
        while (await r.ReadAsync())
        {
            list.Add(new KnownMachine(r.GetString(0), r.GetString(1), r.GetString(2),
                DateTimeOffset.FromUnixTimeMilliseconds(r.GetInt64(3)), DateTimeOffset.FromUnixTimeMilliseconds(r.GetInt64(4))));
        }
        return list;
    }

    /// <summary>The last tree a machine sent, for showing an offline machine.</summary>
    public async Task<string?> LastTreeAsync(string machineId)
    {
        var path = MachineDbPath(machineId);
        if (!File.Exists(path)) return null;
        await using var db = Open(path);
        await using var cmd = db.CreateCommand();
        cmd.CommandText = "SELECT json FROM tree WHERE id = 1";
        return await cmd.ExecuteScalarAsync() as string;
    }

    /// <summary>
    /// The newest recorded output of a pane, oldest first, at most maxBytes
    /// (STATUS S-07). Only output someone watched was ever recorded (ADR 0003).
    /// </summary>
    public async Task<byte[]> RecentOutputAsync(string machineId, string paneId, int maxBytes = 256 * 1024)
    {
        var path = MachineDbPath(machineId);
        if (!File.Exists(path)) return Array.Empty<byte>();
        await using var db = Open(path);
        await using var cmd = db.CreateCommand();
        cmd.CommandText = "SELECT bytes FROM pane_output WHERE pane_id = $pane ORDER BY rowid DESC";
        cmd.Parameters.AddWithValue("$pane", paneId);
        var chunks = new List<byte[]>();
        var total = 0;
        await using var r = await cmd.ExecuteReaderAsync();
        while (total < maxBytes && await r.ReadAsync())
        {
            var chunk = (byte[])r[0];
            chunks.Add(chunk);
            total += chunk.Length;
        }
        chunks.Reverse();
        var all = chunks.SelectMany(c => c).ToArray();
        return all.Length > maxBytes ? all[^maxBytes..] : all;
    }

    // ---- the single writer ---------------------------------------------------

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        Directory.CreateDirectory(Path.Combine(_options.DataDir, "machines"));
        _hubDb = Open(Path.Combine(_options.DataDir, "hub.db"));
        Exec(_hubDb, """
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS machines (
              id TEXT PRIMARY KEY, host TEXT NOT NULL DEFAULT '', version TEXT NOT NULL DEFAULT '',
              first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL, connected INTEGER NOT NULL DEFAULT 0);
            UPDATE machines SET connected = 0;
            CREATE TABLE IF NOT EXISTS messages (
              id TEXT PRIMARY KEY, ts INTEGER NOT NULL, from_addr TEXT NOT NULL, to_addr TEXT NOT NULL,
              text TEXT NOT NULL, sender TEXT NOT NULL, ok INTEGER NOT NULL, error TEXT);
            CREATE INDEX IF NOT EXISTS messages_ts ON messages (ts);
            """);

        var batch = new List<StoreItem>(1024);
        var reader = _queue.Reader;
        try
        {
            while (await reader.WaitToReadAsync(stoppingToken))
            {
                batch.Clear();
                while (batch.Count < 5000 && reader.TryRead(out var item)) batch.Add(item);
                try { Apply(batch); }
                catch (Exception e) { _log.LogError(e, "store batch of {Count} failed", batch.Count); }
                // Let more items accumulate: one transaction per ~100 ms per machine.
                await Task.Delay(100, stoppingToken);
            }
        }
        catch (OperationCanceledException) { }
        finally
        {
            foreach (var db in _machineDbs.Values) db.Dispose();
            _hubDb.Dispose();
        }
    }

    private void Apply(List<StoreItem> batch)
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        foreach (var item in batch.Where(i => i.Kind == ItemKind.Message)) InsertMessage(item.Message!);
        foreach (var group in batch.Where(i => i.Kind != ItemKind.Message).GroupBy(i => i.MachineId))
        {
            var db = MachineDb(group.Key);
            using var tx = db.BeginTransaction();
            var wroteOutput = false;
            foreach (var item in group)
            {
                switch (item.Kind)
                {
                    case ItemKind.Connected:
                        HubUpsert(item.MachineId, item.A ?? "", item.B ?? "", now, connected: true);
                        Insert(db, tx, "INSERT INTO events (ts, kind) VALUES ($ts, 'connected')", ("$ts", now));
                        break;
                    case ItemKind.Disconnected:
                        HubUpsert(item.MachineId, null, null, now, connected: false);
                        Insert(db, tx, "INSERT INTO events (ts, kind) VALUES ($ts, 'disconnected')", ("$ts", now));
                        break;
                    case ItemKind.Tree:
                        Insert(db, tx, "INSERT INTO tree (id, ts, json) VALUES (1, $ts, $json) ON CONFLICT(id) DO UPDATE SET ts = $ts, json = $json",
                            ("$ts", now), ("$json", item.A!));
                        break;
                    case ItemKind.Output:
                        Insert(db, tx, "INSERT INTO pane_output (ts, pane_id, bytes) VALUES ($ts, $pane, $bytes)",
                            ("$ts", now), ("$pane", item.A!), ("$bytes", item.Bytes!));
                        wroteOutput = true;
                        break;
                }
            }
            tx.Commit();
            if (wroteOutput) EnforceCap(db, group.Key);
        }
    }

    /// <summary>Keep the machine's file under the cap by dropping its oldest output.</summary>
    private void EnforceCap(SqliteConnection db, string machineId)
    {
        for (var round = 0; round < 8 && SizeOf(db) > _options.MaxBytesPerMachine; round++)
        {
            Exec(db, """
                DELETE FROM pane_output WHERE rowid IN (
                  SELECT rowid FROM pane_output ORDER BY rowid LIMIT MAX(1, (SELECT COUNT(*) FROM pane_output) / 4));
                DELETE FROM events WHERE rowid IN (
                  SELECT rowid FROM events ORDER BY rowid LIMIT MAX(0, (SELECT COUNT(*) FROM events) - 1000));
                PRAGMA wal_checkpoint(TRUNCATE);
                PRAGMA incremental_vacuum;
                """);
        }
        if (SizeOf(db) > _options.MaxBytesPerMachine)
        {
            _log.LogWarning("machine {Machine} store is still over its cap", machineId);
        }
    }

    private static long SizeOf(SqliteConnection db)
    {
        using var cmd = db.CreateCommand();
        cmd.CommandText = "SELECT (SELECT page_count FROM pragma_page_count()) * (SELECT page_size FROM pragma_page_size())";
        return Convert.ToInt64(cmd.ExecuteScalar());
    }

    private void HubUpsert(string id, string? host, string? version, long now, bool connected)
    {
        using var cmd = _hubDb!.CreateCommand();
        cmd.CommandText = """
            INSERT INTO machines (id, host, version, first_seen, last_seen, connected)
            VALUES ($id, COALESCE($host, ''), COALESCE($version, ''), $now, $now, $c)
            ON CONFLICT(id) DO UPDATE SET
              host = COALESCE($host, host), version = COALESCE($version, version), last_seen = $now, connected = $c
            """;
        cmd.Parameters.AddWithValue("$id", id);
        cmd.Parameters.AddWithValue("$host", (object?)host ?? DBNull.Value);
        cmd.Parameters.AddWithValue("$version", (object?)version ?? DBNull.Value);
        cmd.Parameters.AddWithValue("$now", now);
        cmd.Parameters.AddWithValue("$c", connected ? 1 : 0);
        cmd.ExecuteNonQuery();
    }

    private void InsertMessage(HubMessage m)
    {
        using var cmd = _hubDb!.CreateCommand();
        cmd.CommandText = """
            INSERT OR IGNORE INTO messages (id, ts, from_addr, to_addr, text, sender, ok, error)
            VALUES ($id, $ts, $from, $to, $text, $sender, $ok, $error)
            """;
        cmd.Parameters.AddWithValue("$id", m.Id);
        cmd.Parameters.AddWithValue("$ts", m.Ts.ToUnixTimeMilliseconds());
        cmd.Parameters.AddWithValue("$from", m.From);
        cmd.Parameters.AddWithValue("$to", m.To);
        cmd.Parameters.AddWithValue("$text", m.Text);
        cmd.Parameters.AddWithValue("$sender", m.Sender);
        cmd.Parameters.AddWithValue("$ok", m.Ok ? 1 : 0);
        cmd.Parameters.AddWithValue("$error", (object?)m.Error ?? DBNull.Value);
        cmd.ExecuteNonQuery();
    }

    private SqliteConnection MachineDb(string machineId)
    {
        if (_machineDbs.TryGetValue(machineId, out var db)) return db;
        db = Open(MachineDbPath(machineId));
        Exec(db, """
            PRAGMA auto_vacuum = INCREMENTAL;
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS tree (id INTEGER PRIMARY KEY CHECK (id = 1), ts INTEGER NOT NULL, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS pane_output (ts INTEGER NOT NULL, pane_id TEXT NOT NULL, bytes BLOB NOT NULL);
            CREATE INDEX IF NOT EXISTS pane_output_pane ON pane_output (pane_id);
            CREATE TABLE IF NOT EXISTS events (ts INTEGER NOT NULL, kind TEXT NOT NULL);
            """);
        _machineDbs[machineId] = db;
        return db;
    }

    private string MachineDbPath(string machineId) =>
        Path.Combine(_options.DataDir, "machines", SafeName().Replace(machineId, "_") + ".db");

    [GeneratedRegex("[^A-Za-z0-9._-]")]
    private static partial Regex SafeName();

    private static SqliteConnection Open(string path)
    {
        var db = new SqliteConnection(new SqliteConnectionStringBuilder { DataSource = path, Pooling = false }.ToString());
        db.Open();
        return db;
    }

    private static void Exec(SqliteConnection db, string sql)
    {
        using var cmd = db.CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }

    private static void Insert(SqliteConnection db, SqliteTransaction tx, string sql, params (string Name, object Value)[] args)
    {
        using var cmd = db.CreateCommand();
        cmd.Transaction = tx;
        cmd.CommandText = sql;
        foreach (var (name, value) in args) cmd.Parameters.AddWithValue(name, value);
        cmd.ExecuteNonQuery();
    }

    private enum ItemKind { Connected, Disconnected, Tree, Output, Message }

    private sealed record StoreItem(string MachineId, ItemKind Kind, string? A, string? B, byte[]? Bytes, HubMessage? Message = null);
}

/// <summary>A row of hub.db messages (ADR 0009). Sender is who really called: "user:&lt;name&gt;" or "machine:&lt;id&gt;".</summary>
public sealed record HubMessage(string Id, DateTimeOffset Ts, string From, string To, string Text, string Sender, bool Ok, string? Error);

public sealed record KnownMachine(string Id, string Host, string Version, DateTimeOffset FirstSeen, DateTimeOffset LastSeen);
