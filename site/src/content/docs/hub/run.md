---
title: Run the hub
description: klarkhub is one container on one port with its state on a volume. Docker, compose, sizing.
---

The hub (klarkhub) is a C# Blazor Server app in `hub/`. People sign in to it and
watch and answer the agents on every linked machine. It is a separate server:
deploy it once, then link any number of machines to it.

:::note
This page is about the released hub, the C# Blazor Server one in `hub/`. A rewrite as a Go API with a React UI,
still one container on port 7470, is in progress in `hub-go/` and is not released. Machines keep working with
either, because the machine link does not change. Reasons: ADR 0014.
:::

- One container, one port: plain HTTP on **7470**.
- All state in `/data`: `hub.db`, one file per machine, temporary assets, keys.
- Run **exactly one replica**. SQLite and the asset store are local files.
- Machines dial out to the hub. Nothing listens on the machines.

## Start the hub

Pick one. All three end at the same place: a hub on port 7470 that you open in a browser.

### Docker, the published image

CI publishes a multi-arch image (linux/amd64 and linux/arm64) to GitHub's registry:
`ghcr.io/muthuishere/klarkhub:latest` for each tagged release, `:edge` for builds of other pushes.

```sh
docker run -d --name klarkhub -p 7470:7470 -v klarkhub-data:/data \
  ghcr.io/muthuishere/klarkhub:latest
curl http://localhost:7470/healthz        # "ok"
```

Or build it yourself from `hub/Dockerfile`:

```sh
docker build -t klarkhub hub
docker run -d --name klarkhub -p 7470:7470 -v klarkhub-data:/data klarkhub
```

The image sets `ASPNETCORE_URLS=http://+:7470` and `Hub__DataDir=/data`, declares
`VOLUME /data`, and exposes 7470. Keep `/data` on a volume: it holds `hub.db`, the
per-machine files, assets and keys.

Open `http://localhost:7470` and [sign in as admin](/hub/sign-in/). The first-start
admin password is `adminadmin` (or the value of `KLARK_HUB__ADMIN__PASSWORD`); change it when the banner asks.

### Docker Compose

`hub/docker-compose.yml` builds the image, keeps `/data` in the named volume `hub-data`, and mounts
`hub/appsettings.Production.json` read-only, so create that file first (it can be `{}`). Sign-in defaults to **LDAP**.

```sh
cd hub
echo '{}' > appsettings.Production.json
KLARK_HUB_AUTH_MODE=MockLdap docker compose up -d --build     # mock users, no directory needed
docker compose --profile ldap up -d --build                   # or: a test OpenLDAP (alice/alice, bob/bob)
```

The compose file passes these through (the left name is the shell variable, the right is the hub setting):

| shell variable | hub setting | default in compose |
|---|---|---|
| `KLARK_HUB_AUTH_MODE` | `KLARK_HUB__AUTH__MODE` | `Ldap` |
| `KLARK_HUB_LDAP_HOST` | `KLARK_HUB__AUTH__LDAP__HOST` | `ldap` |
| `KLARK_HUB_LDAP_PORT` | `KLARK_HUB__AUTH__LDAP__PORT` | `1389` |
| `KLARK_HUB_LDAP_USERDN` | `KLARK_HUB__AUTH__LDAP__USERDNTEMPLATE` | `cn={0},ou=users,dc=klark,dc=local` |
| `KLARK_HUB_LDAP_GROUPBASE` | `KLARK_HUB__AUTH__LDAP__GROUPSEARCHBASE` | `dc=klark,dc=local` |
| `KLARK_HUB_JWT_KEY` | `KLARK_HUB__JWT__KEY` | empty (a random key in `/data/jwt.key`) |

### From source

```sh
cd hub
dotnet run
```

The hub listens on `http://localhost:7470`, reachable from this machine only. To let other
machines connect, listen on every interface:

```sh
ASPNETCORE_URLS=http://0.0.0.0:7470 dotnet run
```

Needs the .NET 10 SDK. Data goes to `hub/data/` unless `KLARK_HUB__DATADIR` is set.

With nothing configured the hub uses its built-in mock directory
(`user1`/`user1pass`, `user2`/`user2pass`). The built-in `admin` account (`adminadmin`) works in every mode.

### Use your LDAP or Active Directory

Add the LDAP settings as environment variables (`KLARK_` prefix, `__` between levels):

```sh
docker run -d --name klarkhub -p 7470:7470 -v klarkhub-data:/data \
  -e KLARK_HUB__AUTH__LDAP__HOST=ldap.corp.example.com \
  -e KLARK_HUB__AUTH__LDAP__USERDNTEMPLATE='{0}@corp.example.com' \
  ghcr.io/muthuishere/klarkhub:latest
```

All the LDAP settings are on [LDAP and Active Directory](/hub/ldap/), or switch the
directory later in the UI at `/admin/directory`.

### Environment variables

The hub reads variables with the `KLARK_` prefix, and also unprefixed ones such as the Dockerfile's
`Hub__DataDir` and `ASPNETCORE_URLS`. `Hub:Auth:Ldap:Host` is `KLARK_HUB__AUTH__LDAP__HOST`.
The full list is in [Hub configuration](/reference/hub-config/). The ones you reach for first:

| variable | meaning |
|---|---|
| `ASPNETCORE_URLS` | the address to listen on (`http://+:7470` in the image) |
| `Hub__DataDir` or `KLARK_HUB__DATADIR` | where state lives (`/data` in the image) |
| `KLARK_HUB__AUTH__MODE`, `KLARK_HUB__AUTH__LDAP__*` | sign-in; see [LDAP](/hub/ldap/) |
| `KLARK_HUB__ADMIN__PASSWORD` | the built-in admin's first-start password |
| `KLARK_HUB__PUBLICURL` | the address people open, used in the links `klark hub connect` prints |
| `Hub__Brand__Name`, `Hub__Brand__Tagline`, `Hub__Brand__Logo` | [your own name, tagline and logo](/reference/hub-config/#branding) |
| `Hub__AgentNote` | [the line the hub adds to messages](/reference/hub-config/#the-agent-note) |

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
- [What the hub chat gives you](/app/chat/)
