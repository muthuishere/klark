CREATE TABLE machine_stats (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  machine_id TEXT NOT NULL,
  machine_name TEXT NOT NULL DEFAULT '',
  ts INTEGER NOT NULL,                -- unix milliseconds (the machine's ts_ms when sent, else the hub's clock)
  rss_bytes INTEGER NOT NULL DEFAULT 0,
  peak_rss_bytes INTEGER NOT NULL DEFAULT 0,
  cpu_percent REAL NOT NULL DEFAULT 0,
  uptime_s INTEGER NOT NULL DEFAULT 0,
  version TEXT NOT NULL DEFAULT '',
  mem_total_bytes INTEGER NOT NULL DEFAULT 0,
  mem_available_bytes INTEGER NOT NULL DEFAULT 0,
  load1 REAL NOT NULL DEFAULT 0,
  cpus INTEGER NOT NULL DEFAULT 0,
  panes INTEGER NOT NULL DEFAULT 0,
  agents INTEGER NOT NULL DEFAULT 0,
  agents_json TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX machine_stats_machine_ts ON machine_stats (machine_id, ts);
CREATE INDEX machine_stats_ts ON machine_stats (ts);
