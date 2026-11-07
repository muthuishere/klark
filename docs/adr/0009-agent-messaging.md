# ADR 0009: agents message agents: across sessions and across machines

- Status: accepted (owner request, 2026-11-06). Implements STATUS S-25.
- Extends ADR 0002 (protocol v2) and ADR 0004 (user JWTs).

## Context
An agent running in one klark pane should be able to send a message to another
agent: in the same session, in another klark session on the same machine, or
on another machine linked to the same hub. The receiving agent sees the
message as a prompt and can reply the same way. The `klark msg` CLI does this,
and so does the `klark` skill (`skills/klark`, formerly the separate `klark-msg` skill), which wraps the CLI.

## Addresses
`<machine>/<session>/<agent>`:
- `agent`: the agent's name (`klark agent rename`), or a pane id.
- `session`: a klark session name; the default session is `default`.
- `machine`: the name the hub knows the machine by (`klark expose --name`, or
  `KLARK_MACHINE_NAME`, or the host name).

Short forms resolve locally: `agent` searches every running session on this
machine (it fails if the name is ambiguous); `session/agent` names the
session. A full address whose machine is this machine is also delivered
locally, with no hub.

## Delivery
The receiving agent gets one prompt (`agent.prompt`):

```
[klark msg from alice-laptop/default/reviewer] <text>
(reply: klark msg reply "<text>")
```

Delivery fails, and the sender is told, if the target agent is `blocked`
(agent.prompt rejects it) or does not exist. Every send and receipt is appended
to `<klark data dir>/messages.jsonl`. The receiver also records the sender per
pane, so `klark msg reply` knows whom to answer.

## Same machine (no hub)
`klark msg` opens each session's API socket (`sessions/<name>/klark.sock`) and
calls `agent.list` and `agent.prompt` itself. No network is involved.

## Across machines (through the hub)
Messages are machine-initiated. ADR 0002 has only the hub pulling, so this is
an exception, and it does not use the link socket: the sender's CLI makes a
plain HTTP call to the hub. The hub then pushes to the target machine over the
target's existing link.

Hub HTTP API (`http://<hub>:8080`, the same host as `ws://<hub>:8080/agent`).
Every call takes `Authorization: Bearer <token>`, where the token is either:
- a **machine token**, the same one `klark expose` uses (the caller acts as that machine); or
- a **user JWT** (see below). The user must be able to see the target machine
  and type into it (ADR 0004 groups).

| call | body / query | answer |
|---|---|---|
| `POST /api/token` | `{username, password}` (JSON or form) | `{token, expires_at, user, groups}` |
| `GET /api/agents` | (none) | `[{machine, session, agent, name, pane_id, status, address}]` for every online machine the caller may see |
| `POST /api/messages` | `{to, text, from?}` | `200 {id, delivered:true}`, or 4xx `{error}` |
| `GET /api/messages?limit=100` | (none) | the most recent messages the caller may see |

Hub → machine, link protocol **v2** (the hub still accepts v1 links, but they
cannot receive messages):

| message | machine reply |
|---|---|
| `list_agents {id}` | `result {id, result: [{session, name, agent, pane_id, status}]}` |
| `deliver {id, session, agent, from, text}` | `result {id, result:{delivered:true}}` or `result {id, error}` |

The link carries these out by running its own `klark` binary:
`klark msg ls --local --json` and `klark msg deliver --session <s> <agent> --from <addr> <text>`.
So the same code delivers local and remote messages.

The hub stores messages in `hub.db`:
`messages(id, ts, from_addr, to_addr, text, sender, ok, error)`.

## User JWTs
- Sign-in stays the Blazor login page (cookie). After signing in, `/token`
  shows a JWT for the CLI, with a copy button.
- `POST /api/token` issues the same token directly. It goes through the
  configured directory (LDAP by default), with the same rate limit as the
  login form.
- HS256 with a key from `Hub:Jwt:Key` (`KLARK_HUB__JWT__KEY`). If that is not
  set, a random key is created once and kept in `<DataDir>/jwt.key`. Lifetime
  is `Hub:Jwt:Hours` (default 12). The claims are name, display name and groups.
- `klark msg login --hub <url> --user <name>` asks for the password, and saves
  the JWT in `<klark config dir>/hub.json` (mode 0600).

## Sign-in modes (amends ADR 0004)
- With no `Hub:Auth:Mode` configured, the hub uses **`Ldap`** when
  `Hub:Auth:Ldap:Host` is set and **`MockLdap`** otherwise, so an unconfigured
  hub works locally out of the box. `Ldap`: the organisation's LDAP or
  Active Directory checks every password, by a bind as the user; the hub
  stores no directory passwords. Two styles are supported: direct bind
  (`UserDnTemplate`, including the AD UPN form `{0}@corp.example.com`) and
  search-then-bind (`UserSearchBase`, `UserFilter`, `BindDn`/`BindPassword`).
  LDAPS (`UseSsl`) and StartTLS are supported, and groups come from a group
  search and/or `memberOf`.
- **`MockLdap`** users live in hub.db with PasswordHasher hashes (first start
  seeds `user1`/`user1pass`, `user2`/`user2pass`, or `Hub:Auth:MockUsers` if
  configured). The admin manages them at `/admin/users`; users change their own
  password at `/account/password`.
- A **built-in `admin`** account always exists (hash in hub.db; first-start
  password `adminadmin`, or `Hub:Admin:Password`). It is checked before any
  directory, works in every mode, and sees and drives every machine. While its
  password is the default the UI shows a banner to change it. The name
  `admin` is therefore reserved and never reaches the directory.
- The admin can switch the mode and edit LDAP settings at `/admin/directory`
  (stored in hub.db, effective on the next sign-in, no restart). Configuration
  stays the default for anything not stored. "Test connection" binds with a
  typed test user and password against the saved settings.
- `Local` is unchanged.

## Connecting a machine (device flow)
- `klark hub connect --url <hub>` calls `POST /api/connect/start {name}` and
  gets a code like `ABCD-EFGH`, a secret `poll_id` and a `/connect?code=` link
  (base from `Hub:PublicUrl`, else the request with X-Forwarded-* honoured).
- A signed-in person opens the link and approves or denies. Any signed-in user
  may approve; they are recorded as the machine's owner (`approved_by`), and
  only they or an admin may re-key a machine of that name.
- The machine polls `POST /api/connect/poll {poll_id}` every 2 s; on approval
  it receives a fresh 32-byte token exactly once. Codes expire after 10
  minutes. Pending requests live in memory (single replica); only the token's
  SHA-256 is stored, in hub.db next to `Hub:Machines`, and both are accepted.
- Admins list and revoke machines at `/admin/machines`; revoking drops the
  live link.
- Hub settings can be given as environment variables with the prefix
  `KLARK_`, e.g. `KLARK_HUB__AUTH__LDAP__HOST`.

## Consequences
- Agents on any linked machine can reach each other with one command, and any
  user with a JWT can script the same thing.
- A machine token can message every machine on its hub, because one hub is
  one trust domain. Restricting this per group would need a further decision.
- Plain HTTP (ADR 0005) means message text crosses the network in clear text,
  just like pane output already does.
