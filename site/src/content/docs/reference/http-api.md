---
title: HTTP API
description: Tokens, agents, messages, assets, files and the machine connect flow.
---

Base URL: the hub, for example `http://hub.lan:7470`. JSON uses `snake_case`.

## Authentication

Every `/api` call except `/api/token` and `/api/connect/*` takes
`Authorization: Bearer <token>`. The token is either:

- a **user JWT** (from `/api/token` or the `/token` page). The user's groups
  decide what they see and where they may type;
- a **machine token**. The caller acts as that machine and may reach every
  machine on the hub. Fleet tokens also send `X-Klark-Machine`.

No token or a bad one gives `401`.

## Tokens

### `POST /api/token`

Body (JSON or form): `{username, password}`. Same directory and same 10 per
minute limit as the sign-in page.

```sh
curl -s -X POST http://hub.lan:7470/api/token \
  -H 'content-type: application/json' \
  -d '{"username":"user1","password":"user1pass"}'
```

```json
{"token":"eyJ…","expires_at":"2026-11-08T03:46:45+00:00","user":"user1","groups":["klark-users"]}
```

`401` on a bad password.

## Agents and messages

### `GET /api/agents`

Every agent on every online machine you may see. Machines are asked
concurrently, 10 s each.

```json
[{"machine":"laptop","session":"work","agent":"claude","name":"reviewer","pane_id":"w1:p1","status":"idle","address":"laptop/work/reviewer"}]
```

### `POST /api/messages`

Body: `{to, text, from?}`. `to` is `machine/session/agent`.

| status | meaning |
|---|---|
| `200 {id, delivered: true}` | delivered |
| `400` | bad address or empty text |
| `413` | text over 16 KB |
| `403` | you may not see that machine or type into it |
| `404` | machine not online |
| `409` | machine too old to receive messages |
| `502` | the machine refused (no such agent, or it is blocked) |
| `504` | no answer within 10 s |

`from` defaults to `user:<name>` or the machine name. The hub also records who
really called, whatever `from` says.

### `GET /api/messages?limit=100`

The newest messages (at most 500) whose source or target machine you may see.

## Images and files

### `POST /api/panes/{machine}/{pane}/images?name=&caption=`

User JWT, raw image as the body. The hub stores it and the machine fetches it
and prompts the agent with its path.

`200 {sha256, size, mime, path}`; `403` read-only; `404` offline; `409` machine
too old; `413` over 20 MB; `415` not an image.

### `PUT /api/assets?pane=<pane>&name=<name>`

Machine token, raw file as the body (what `klark offer` uses).
`200 {sha256, size, mime, url}`; `413` over 100 MB.

### `GET /api/assets/{sha256}`

Machine token. The bytes, only for an asset sent to that machine.

### `GET /assets/{sha256}`

Cookie or JWT. Any asset on a machine you may see.

### `GET /api/files/{machine}?pane=&path=`

Signed-in user. File metadata, fetched from the machine. Also
`/api/files/{machine}/raw` (bytes; `&download=true` for an attachment) and
`/api/files/{machine}/markdown` (rendered preview). See
[safety rules](/app/files/#safety-rules).

## Connecting a machine

Used by `klark hub connect`. No token needed.

### `POST /api/connect/start`

```sh
curl -s -X POST http://hub.lan:7470/api/connect/start \
  -H 'content-type: application/json' -d '{"name":"laptop"}'
```

```json
{"code":"8PYA-BBBM","poll_id":"…","url":"http://hub.lan:7470/connect?code=8PYA-BBBM","expires_in":600,"interval":2}
```

A signed-in person opens `url` and approves or denies.

### `POST /api/connect/poll`

Body `{poll_id}`, every `interval` seconds. Answers `pending`, `denied`,
`expired`, or once only `{status: "approved", token, machine}`.

## Health

`GET /healthz` → `"ok"`.

## Machine link

`/agent`, `/agent/session`, `/agent/events` and `/agent/push` are the machine
link. See [Link protocol](/reference/link-protocol/).
