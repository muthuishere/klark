CREATE TABLE machine_stats (
  id BIGSERIAL PRIMARY KEY,
  machine_id TEXT NOT NULL,
  machine_name TEXT NOT NULL DEFAULT '',
  ts BIGINT NOT NULL,
  rss_bytes BIGINT NOT NULL DEFAULT 0,
  peak_rss_bytes BIGINT NOT NULL DEFAULT 0,
  cpu_percent DOUBLE PRECISION NOT NULL DEFAULT 0,
  uptime_s BIGINT NOT NULL DEFAULT 0,
  version TEXT NOT NULL DEFAULT '',
  mem_total_bytes BIGINT NOT NULL DEFAULT 0,
  mem_available_bytes BIGINT NOT NULL DEFAULT 0,
  load1 DOUBLE PRECISION NOT NULL DEFAULT 0,
  cpus BIGINT NOT NULL DEFAULT 0,
  panes BIGINT NOT NULL DEFAULT 0,
  agents BIGINT NOT NULL DEFAULT 0,
  agents_json TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX machine_stats_machine_ts ON machine_stats (machine_id, ts);
CREATE INDEX machine_stats_ts ON machine_stats (ts);
