---
title: Run the hub
description: klarkhub is one container on one port with its state on a volume. Docker, compose, sizing.
---

The hub (klarkhub) is a Go server with a React UI embedded, in `hub-go/`. People sign in to it and
watch and answer the agents on every linked machine. It is a separate server:
deploy it once, then link any number of machines to it.

- One container, one port: plain HTTP on **7470**.
- All state in `/data`: `hub.db` (SQLite), `session.key`, and by default the backups in `/data/backup`.
- Run **exactly one replica**. SQLite is a local file.
- Machines dial out to the hub. Nothing listens on the machines.

## Start the hub

Pick one. All three end at the same place: a hub on port 7470 that you open in a browser.

### Docker, the published image

CI publishes a multi-arch image (linux/amd64 and linux/arm64) to GitHub's registry:
`ghcr.io/muthuishere/klarkhub:latest` for each tagged release, `:edge` for builds of other pushes.

```sh
docker run -d --name klarkhub -p 7470:7470 -v klarkhub-data:/data \
  -e HUB_ADMIN_PASSWORD='a long password' \
  ghcr.io/muthuishere/klarkhub:latest
curl http://localhost:7470/healthz        # "ok"
```

Or build it yourself from `hub-go/Dockerfile`:

```sh
docker build -t klarkhub hub-go
docker run -d --name klarkhub -p 7470:7470 -v klarkhub-data:/data klarkhub
```

The image is distroless and runs as non-root, sets `HUB_ADDR=:7470` and `HUB_DATA_DIR=/data`,
declares `VOLUME /data`, and exposes 7470. Keep `/data` on a volume.

Open `http://localhost:7470` and [sign in as admin](/hub/sign-in/) with `adminadmin`, or the value
of `HUB_ADMIN_PASSWORD`.

### Docker Compose

`hub-go/docker-compose.yml` builds the image and keeps `/data` in a named volume. It sets
`HUB_ADMIN_PASSWORD` (change it) and lists the other settings as comments.

```sh
cd hub-go
docker compose up -d --build
HUB_BACKUP=s3 HUB_BACKUP_PATH=klarkhub docker compose --profile s3 up -d   # with a local S3 (SeaweedFS) for backups
```

### From source

```sh
cd hub-go/ui && npm ci && npm run build     # the binary embeds the built UI
cd .. && make build && ./bin/klarkhub
```

The hub listens on `:7470`. Needs Go and Node. Data goes to `./data` unless `HUB_DATA_DIR` is set.
With nothing configured only the built-in `admin` and `HUB_USERS` sign in.

### Use your LDAP or Active Directory

```sh
docker run -d --name klarkhub -p 7470:7470 -v klarkhub-data:/data \
  -e HUB_LDAP_HOST=ldap.corp.example.com \
  -e HUB_LDAP_USER_DN_TEMPLATE='{0}@corp.example.com' \
  ghcr.io/muthuishere/klarkhub:latest
```

All the LDAP settings are on [LDAP and Active Directory](/hub/ldap/), or switch the
directory later in the UI at `/admin/directory`.

### Environment variables

The full list is in [Hub configuration](/reference/hub-config/). The ones you reach for first:

| variable | meaning |
|---|---|
| `HUB_ADDR` | the address to listen on (`:7470`) |
| `HUB_DATA_DIR` | where state lives (`/data` in the image) |
| `HUB_ADMIN_PASSWORD` | the built-in admin's password (default `adminadmin`; set it) |
| `HUB_AUTH_MODE`, `HUB_LDAP_*` | sign-in; see [LDAP](/hub/ldap/) |
| `HUB_PUBLIC_URL` | the address people open, used in the links `klark hub connect` prints |
| `HUB_BASE_PATH` | mount prefix behind a reverse proxy, e.g. `/klark` |
| `HUB_BACKUP`, `HUB_BACKUP_PATH` | where SQLite backups go (a folder or S3) |

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

Measured on the old Blazor hub image in experiment E-06, with the 100 and 500 machine
rows extrapolated from per-unit costs. The Go hub has not been load tested yet.

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
