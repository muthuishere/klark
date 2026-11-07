---
title: FAQ and troubleshooting
description: Socket path limits, proxies, old protocol versions, ports, and common errors.
---

## klark exits with "sun_path" in the error

```text
Error: Custom { kind: InvalidInput, error: "local socket name length exceeds capacity of sun_path of sockaddr_un" }
```

Each session's socket lives in klark's config directory
(`sessions/<name>/klark.sock`). Unix sockets have a short path limit (104 bytes
on macOS, 108 on Linux). A deep `XDG_CONFIG_HOME` or `KLARK_CONFIG_PATH`, or a
very long session name, goes over it. Use a shorter config path or a shorter
session name (`klark -s api` instead of a long folder name).

## A proxy in front of the hub

The hub needs no WebSockets, but its streams must not be buffered:

- **Turn off response buffering.** The hub sends `X-Accel-Buffering: no` on the
  machine event stream, which nginx honours. Other proxies need it set.
- **Allow responses to stay open 60 s or more.** `/agent/events` is held open
  with a ping line every 15 s. The browser's `/_blazor` uses long polling.
- **Set `KLARK_HUB__TRUSTFORWARDEDHEADERS=true`** so links and rate limits use
  the real client address.
- **Set `KLARK_HUB__PUBLICURL`** to the address people open.

Symptoms of buffering: machines connect but stay offline, panes never update,
or the page shows "reconnecting".

## "unsupported protocol N; hub speaks 4-5"

The machine runs an older klark than the hub accepts. Versions 1 to 3 used
WebSockets and cannot link to a current hub. Rebuild klark on that machine.

Related messages: the file viewer says **"update klark"** for a machine below
protocol 5. Messaging answers `409` for a machine too old to receive messages.

## Which ports?

| port | what |
|---|---|
| 8080 | the hub, plain HTTP, inside the container |
| 30080 | the example Kubernetes NodePort |
| none | klark machines: they only dial out |

Change the host port with `docker run -p 9000:8080 …`. From source, use
`ASPNETCORE_URLS=http://0.0.0.0:9000 dotnet run`.

## The machine shows "offline" in the hub

- Is `klark hub connect` (or `klark expose`) still running on it?
  `klark hub status` tells you.
- Was it revoked at `/admin/machines`? Then run `klark hub connect --url …` again
  and approve it.
- A machine goes offline 45 s after its event stream ends.

## I cannot sign in

- `admin` works in every mode. Its first password is `adminadmin`, or the
  value of `KLARK_HUB__ADMIN__PASSWORD` on the first start.
- `docker compose up` defaults to LDAP mode with a host called `ldap`. Without
  the `ldap` profile only `admin` can sign in. Use
  `KLARK_HUB_AUTH_MODE=MockLdap docker compose up -d --build` for mock users.
- After 10 failed attempts in a minute, wait a minute.

## "no hub configured"

`klark msg`, `klark offer` and `klark expose url` need a hub. Run
`klark hub connect --url …` once, or set `KLARK_HUB_URL`.

## The terminal looks garbled

The hub draws the terminal at the pane's real size and scales it. If you are
on a very old machine link, update klark. Otherwise use the chat view, which
reflows text for any width.

## Where are the logs?

- klark: `~/.config/klark/klark.log`, `klark-client.log`, `klark-server.log`.
- Hub: the container's stdout (`docker logs klarkhub`).

## Does klark phone home?

No. There is no self-update, no hosted announcements and no remote catalog. The
hub bundles its own scripts (xterm, highlight.js) and works on a LAN without
internet access.
