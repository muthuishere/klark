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
  `~`; the default is your home folder. `KLARK_HOOK` is set to the hook's name.
- **Logs.** Each hook's output goes to `<config>/hooks/<name>.log` (one rotation
  at 5 MB). The supervisor's own log is `<config>/hooks/supervisor.log`.
  Environment values are never written to a log or shown.
- `klark config check` reports bad names, duplicates, empty commands and a
  `restart` on a stop hook; such a hook is not run.

## Commands

```
klark hooks list [--json]          every hook: state, pid, restarts, last exit, uptime
klark hooks start [<name>]         start one hook; with no name, start them all and keep
                                   them running even with no klark server
klark hooks stop [<name>]          stop one hook; with no name, stop all and run the stop hooks
klark hooks restart [<name>]       restart one hook, or all
klark hooks logs <name> [-n N] [-f]
```

The global menu's **jobs** panel has a **hooks** tab with the same controls
(`s` start, `x` stop, `e` restart, `r` refresh; `tab` switches to feeds).

## Telling an agent

A hook's output can feed an agent:

```
klark feed add ops "tell me when the proxy logs an error or restarts" --hook os-proxy
```

This follows the hook's log (`--follow <config>/hooks/os-proxy.log`).
