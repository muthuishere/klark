# Hooks: machine services klark starts and stops

Services that belong to the agent workspace (a dashboard, its auth proxy, a
tunnel, a local MCP server, small fetchers) can be run by klark instead of
launchd jobs, cron entries or tmux loops.

```toml
# ~/.config/klark/config.toml

[[hooks.start]]          # runs once per machine, with the first klark server
name = "os-dashboard"
run  = "bun run serve"
cwd  = "~/work/company-os"
restart = "always"       # always (default) | on-failure | never
env  = { PORT = "8788" }

[[hooks.start]]
name = "os-proxy"
run  = "sec run PROXY_SECRET -- node auth-proxy.mjs"   # secrets through a wrapper
cwd  = "~/work/one-os/skills/cloudflare-tunnel"
notify = ["ops", "work/reviewer"]   # agents told when it stops running (optional)

[[hooks.stop]]           # runs once, after the last klark server stops
name = "flush"
run  = "./scripts/flush.sh"
```

## How it runs

- **Global, not per session.** The first klark server to start launches one
  supervisor for the machine; more sessions do not start the hooks again.
  Stopping one session (`klark server stop`) leaves them running while any other
  session runs. About five seconds after the last server stops, the supervisor
  stops the start hooks and runs the stop hooks (each up to two minutes), then
  ends.
- **Supervised.** A start hook that ends is started again by its `restart`
  policy, after 1 s, then 2, 4, … up to 60 s; the wait resets after a run of a
  minute or more. A hook stopped by hand stays stopped until started again.
- **Commands** run through your shell (`$SHELL -c`, `cmd /C` on Windows) in their
  own process group, so stopping one stops everything it started. `cwd` accepts
  `~`; the default is your home folder. `KLARK_HOOK` is set to the hook's name,
  `KLARK_ENV=1` and `KLARK_BIN_PATH` too, so a hook can use klark: it belongs to no
  session, and reaches all of them with `klark session list --json` and each
  session's socket.
- **`restart = "on-failure"`** treats an exit 0 as a clean stop: the hook stays
  `exited` until `klark hooks start <name>` or `restart <name>` runs it again.
  It is still announced (see Notices below): a service that exits 0 is just as
  offline.
- **Logs.** Each hook's output goes to `<config>/hooks/<name>.log` (one rotation
  at 5 MB). The supervisor's own log is `<config>/hooks/supervisor.log`.
  Environment values are never written to a log or shown.
- `klark config check` reports bad names, duplicates, empty commands and a
  `restart` on a stop hook; such a hook is not run.

## Notices: when a hook stops running

A hook that dies prints nothing, so following its output alone never says it
died. Every time a start hook leaves `running` on its own (it exits with any
code, a clean exit 0 under `on-failure` included, is killed, or cannot start),
the supervisor writes one line to the hook's log and sends it to every agent in
the hook's `notify` list:

```
klark hook dmcp: exited (code 0) at 2026-11-17 08:12:40 UTC; restart policy on-failure; not restarting (klark hooks start dmcp runs it again). Logs: klark hooks logs dmcp
klark hook os-proxy: exited (code 1) at 2026-11-17 08:12:40 UTC; restart policy always; restarting in 1s (further quick exits are not reported until it stays up 10s). Logs: klark hooks logs os-proxy
klark hook os-proxy: running again at 2026-11-17 08:13:02 UTC (pid 4812, 3 restarts). Logs: klark hooks logs os-proxy
```

- **One notice per state change.** In a restart loop only the first exit is
  reported. Once the hook has stayed up 10 s, one "running again" notice goes
  out and its next exit is reported again. A loop that ends without a restart
  (`on-failure` after an exit 0, `never`) is reported too. `klark hooks list`
  counts every restart.
- **Nothing is sent for what you asked for.** `klark hooks stop` and the
  shutdown after the last klark server stops send no notice. A hook you start or
  restart by hand reports its next exit again.
- **`notify`** takes `klark msg send` addresses: `agent`, `session/agent`, or
  `machine/session/agent` (through the hub). A bare `agent` must name exactly
  one agent across the running sessions on this machine; `session/agent` is
  unambiguous. The agent sees
  `[klark msg from <machine>/hooks/<name>] klark hook …`, and the send is in
  `klark msg log`. Failed sends are written to `<config>/hooks/supervisor.log`.
  `klark config check` reports a malformed address, and `notify` on a stop hook.
- **When a `notify` change takes effect.** The supervisor reads `config.toml`
  only when it handles a request (`klark hooks start|stop|restart`). A hook
  that is not running picks up the new list with the next request. A running
  hook keeps the list it was started with until `klark hooks restart <name>`.
- **Feeds.** The notice is a line in the hook's log, so a feed made with
  `--hook <name>` (below) delivers it like any other output. A feed stops
  itself after its `--idle-stop` (default 7 days without output); for a service
  that must never be silently down, `notify` is the path that does not expire.

## Commands

```
klark hooks list [--json]          every hook: state, pid, restarts, last exit, restart
                                   policy, when it last exited, uptime
klark hooks start [<name>]         start one hook; with no name, start them all and keep
                                   them running even with no klark server
klark hooks stop [<name>]          stop one hook; with no name, stop all and run the stop hooks
klark hooks restart [<name>]       restart one hook, or all
klark hooks logs <name> [-n N] [-f]
```

`klark hooks list --json` works from any shell, with or without a klark server:
`{"supervisor": <pid or null>, "pinned": bool, "hooks": [{"name", "kind", "state",
"pid", "restarts", "last_exit", "started_at", "exited_at", "restart", "detail"}]}`.
`started_at` and `exited_at` are Unix seconds (the current run's start, the last
run's end); the table shows `EXITED` as "5m ago". `state` is
`running`, `starting`, `backoff` (waiting to restart), `exited` (ended, not restarted),
`failed` (ended with an error, not restarted), `stopped` (stopped by hand) or
`not running` (no supervisor); stop hooks show `waiting`, `done` or `failed`.

The global menu's **jobs** panel has a **hooks** tab with the same controls
(`s` start, `x` stop, `e` restart, `r` refresh; `tab` switches to feeds).

## Telling an agent

A hook's output can feed an agent:

```
klark feed add ops "tell me when the proxy logs an error or restarts" --hook os-proxy
```

This follows the hook's log (`--follow <config>/hooks/os-proxy.log`), which
also carries the supervisor's notices: `--- klark hooks: start <name>` on each
start, and the "exited" and "running again" lines above.
