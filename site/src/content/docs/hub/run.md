---
title: Run the hub
description: klarkhub is one container on one port with its state on a volume. Docker, compose, sizing.
---

The hub (klarkhub) is a C# Blazor Server app in `hub/`. People sign in to it and
watch and answer the agents on every linked machine. It is a separate server:
deploy it once, then link any number of machines to it.

- One container, one port: plain HTTP on **7470**.
- All state in `/data`: `hub.db`, one file per machine, temporary assets, keys.
- Run **exactly one replica**. SQLite and the asset store are local files.
- Machines dial out to the hub. Nothing listens on the machines.

## Docker

```sh
docker build -t klarkhub hub
docker run -d --name klarkhub -p 7470:7470 -v klarkhub-data:/data klarkhub
curl http://localhost:7470/healthz        # "ok"
```

Open `http://localhost:7470` and [sign in as admin](/hub/sign-in/).

With nothing configured, the hub uses its built-in mock directory
(`user1`/`user1pass`, `user2`/`user2pass`). To use your LDAP or Active Directory,
add `-e KLARK_HUB__AUTH__LDAP__HOST=ldap.corp.example.com` and the other
[LDAP settings](/hub/ldap/), or switch later in the UI.

Settings are environment variables with the `KLARK_` prefix and `__` between
levels. `Hub:Auth:Ldap:Host` is `KLARK_HUB__AUTH__LDAP__HOST`. The full list is
in [Hub configuration](/reference/hub-config/).

## Docker Compose

`hub/docker-compose.yml` builds the image and keeps `/data` in a named volume.
It defaults to **LDAP** sign-in, and it mounts `hub/appsettings.Production.json`,
so create that file first (it can be `{}`).

```sh
cd hub
echo '{}' > appsettings.Production.json
KLARK_HUB_AUTH_MODE=MockLdap docker compose up -d --build     # mock users
docker compose --profile ldap up -d --build                   # or: a test OpenLDAP (alice/alice, bob/bob)
```

## From source

```sh
cd hub
dotnet run
```

Needs the .NET 10 SDK. Data goes to `hub/data/` unless `KLARK_HUB__DATADIR` is set.
`ASPNETCORE_URLS=http://127.0.0.1:17470 dotnet run` picks another port.

## Plain HTTP

The hub speaks plain HTTP. Passwords, tokens and pane output cross the network
unencrypted. Run it on a trusted LAN or VPN. For anything public, put a TLS
proxy in front and use `https://` URLs; klark machines support both. See
[Security model](/reference/security/).

No WebSockets are used, by machines or browsers. Any proxy, ingress, NodePort or
corporate firewall that passes HTTP passes the hub. Two proxy settings matter:
do not buffer responses, and allow a response to stay open for at least 60 s.
Details in the [FAQ](/help/faq/#a-proxy-in-front-of-the-hub).

## Sizing

Measured on the real hub image in experiment E-06, with the 100 and 500 machine
rows extrapolated from per-unit costs.

| machines | viewers | memory limit | vCPU | sockets / fds | machine → hub traffic |
|---|---|---|---|---|---|
| 10 | 10 | 512m (measured: 224 MiB peak with 101 machines) | 0.25 | ~30 / ~80 | ~50 KB/s while one busy pane is watched |
| 50 | 25 | 512m | 0.5 | ~100 / ~300 | ~50–100 KB/s |
| 100 | 50 | 1g | 1 | ~170 / ~500 | ~100–150 KB/s |
| 500 | 100 | 2g | 2 | ~650 / ~2,300 | ~0.5–1 MB/s |

- `--memory 512m --cpus 1` is enough up to about 50 machines and 25 viewers.
- Above about 200 machines, set `--ulimit nofile=65536:65536`.
- An idle machine costs almost nothing: one held HTTP request and a 22-byte
  ping line every 15 s (E-11).

More numbers: [Performance](/reference/performance/).

## Next

- [Kubernetes](/hub/kubernetes/)
- [First sign-in](/hub/sign-in/)
- [Connect a machine](/hub/machines/)
