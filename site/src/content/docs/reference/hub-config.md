---
title: Hub configuration
description: Every HUB_* environment variable, and its default.
---

The hub reads `HUB_*` environment variables. Source of truth: `hub-go/API.md`;
code: `hub-go/internal/config`.

## General

| variable | default | meaning |
|---|---|---|
| `HUB_ADDR` | `:7470` | listen address |
| `HUB_BASE_PATH` | none | mount prefix behind a reverse proxy, e.g. `/klark` |
| `HUB_PUBLIC_URL` | from the request (`X-Forwarded-*`) | public origin, no path; used in `klark hub connect` links and OIDC redirects |
| `HUB_DATA_DIR` | `data` (`/data` in the image) | `hub.db` (SQLite) and `session.key` |
| `HUB_SESSION_KEY` | generated into the data dir | signs the `klark_session` cookie |
| `HUB_ADMIN_PASSWORD` | `adminadmin` | the built-in `admin` account's password; set it |

## Sign-in

| variable | default | meaning |
|---|---|---|
| `HUB_AUTH_MODE` | `local`, or `ldap` when `HUB_LDAP_HOST` is set | `local`, `ldap` or `mockldap`. `/admin/directory` overrides it |
| `HUB_USERS` | empty | `alice=<bcrypt or sha256:hex>[:admin],bob=...`; hash with `klarkhub hash-password` |
| `HUB_MOCK_USERS` | empty (seeds `user1`, `user2`) | only with `mockldap`: `name=password[:admin][:group1\|group2],...` |
| `HUB_LDAP_*` | | see [LDAP and Active Directory](/hub/ldap/#settings) |
| `HUB_OIDC_ISSUER`, `_CLIENT_ID`, `_CLIENT_SECRET`, `_SCOPES`, `_ADMIN_GROUP`, `_NAME` | scopes `openid profile email`, name `ADFS` | ADFS / OpenID Connect (code + PKCE) |
| `HUB_OIDC_PROVIDERS` | empty | more OIDC providers as JSON; see `hub-go/docker-compose.yml` |

## Machines from configuration

Machines approved through `klark hub connect` are stored in `hub.db`.

| variable | meaning |
|---|---|
| `HUB_MACHINES` | `name=<sha256 of token>[,fleet*=<sha256>]`; a name ending in `*` is a fleet token. Hash with `klarkhub hash-token` |

Per-machine access groups are not available yet in the current hub.

## Analytics, storage and backups

| variable | default | meaning |
|---|---|---|
| `HUB_STATS_INTERVAL_SECONDS`, `HUB_STATS_RETENTION_DAYS` | 30 (min 5), 14 | least time between two stats calls to one machine; days kept |
| `HUB_BACKUP` | `none` (the image: `folder`) | `none`, `folder` or `s3`: the one target for SQLite backups and analytics files |
| `HUB_BACKUP_PATH` | (the image: `/data/backup`) | a folder, or a key prefix for S3 |
| `HUB_BACKUP_S3_BUCKET`, `_REGION`, `_ENDPOINT`, `_ACCESS_KEY_ID`, `_SECRET_ACCESS_KEY`, `_PATH_STYLE` | | S3 target |
| `HUB_SQLITE_BACKUP_EVERY`, `HUB_SQLITE_BACKUP_KEEP` | `6h` (min `5m`), `7` | snapshot interval, how many to keep |
| `HUB_POD_ID` | hostname + random | names this hub as the owner of the backup location |

Details: `hub-go/README.md`.

## The agent note

When you send a message from the hub, it adds one short line so the agent names every file it
creates or changes by absolute path. The hub turns those paths into links that open a preview.
The note is fixed in the current hub; it is not a setting.

## Limits

| what | limit |
|---|---|
| image a person sends to an agent | 10 MB |
| largest file the viewer fetches | 10 MB |
| message (`klark msg`) | 16 KB |

Agent-to-person files (`klark offer`) are not available yet in the current hub.

## klark (machine side) environment

| variable | meaning |
|---|---|
| `KLARK_HUB_URL` | hub URL for `expose`, `msg`, `offer` |
| `KLARK_HUB_TOKEN` | machine token |
| `KLARK_HUB_JWT` | user token for `klark msg` |
| `KLARK_MACHINE_NAME` | this machine's name on the hub |
| `KLARK_EXPOSE_AGENTS` | default for `--agents` |
| `KLARK_HUB_CA_FILE` | PEM file with a private CA for `https://` hubs |
| `KLARK_FILES_MAX_BYTES` | lower the machine's 10 MB file-viewer limit |
| `KLARK_CONFIG_PATH` | config file path |
| `KLARK_SOCKET_PATH` | API socket of the current session (set inside panes) |
