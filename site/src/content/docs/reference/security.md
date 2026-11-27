---
title: Security model
description: What the hub trusts, what crosses the network, and what a machine refuses.
---

## Plain HTTP by default

The hub listens on plain HTTP. **Passwords, tokens, messages and pane output
cross the network in clear text.** Run the hub on a trusted LAN or VPN. For
anything else, put a TLS proxy in front and use `https://`; machines verify
certificates with rustls (`KLARK_HUB_CA_FILE` for a private CA).

## One hub is one trust domain

- A **machine token** can message every machine on its hub.
- An **admin** (the built-in `admin`, or `HUB_LDAP_ADMIN_GROUP`) sees and drives
  every machine.
- Every signed-in user sees every machine. Per-machine access groups are not
  available yet in the current hub.

## What a machine accepts from the hub

- Reads (`pane.read`, `agent.read`, `session.snapshot`, …) from an allow-list.
- Text input and keys into a pane.
- Message delivery, image fetches, transcript and file reads.

It never accepts calls that close panes or change layout. With `--agents`,
everything outside the list is hidden and refused.

## Secrets at rest

- Machine tokens: only their SHA-256 is stored in `hub.db`. `klark hub ls` never
  prints a token. `hubs.json` and `hub.json` are mode 0600.
- Directory passwords: never stored. LDAP checks them by a bind.
- Local and mock passwords: bcrypt hashes in `hub.db` (`HUB_USERS` also takes `sha256:`).
- Cookie key: `HUB_SESSION_KEY`, or `<DataDir>/session.key`, so a redeploy does not sign people out.

## Sign-in

- Failures are rate limited per user and per address (see `hub-go/API.md`).
- Set `HUB_ADMIN_PASSWORD`: the default is `adminadmin`.

## Files and assets

- Types are decided from the bytes, never from the name.
- HTML, SVG and XML downloads are served as `application/octet-stream`; all
  responses are `nosniff`; non-images are attachments.
- The file viewer only reads regular files under the pane's folder or `$HOME`,
  never credential-shaped paths, at most 10 MB, checked by both the hub and the
  machine. Responses carry a sandboxing CSP.
- `klark offer` refuses credential-looking paths without `--force`.

## Known risks

- Plain HTTP, as above.
- The mock directory has demo passwords. Use it for trials only.
- Sign-in against a real LDAP server has not been tried yet (STATUS S-28).
