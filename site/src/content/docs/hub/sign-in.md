---
title: First sign-in and users
description: The built-in admin, the mock directory, groups, and who may see and type.
---

## Sign in as admin

1. Open the hub and sign in as **`admin`** / **`adminadmin`**, or the value of
   `HUB_ADMIN_PASSWORD`.
2. The current hub has no change-password page yet: set the password with
   `HUB_ADMIN_PASSWORD` and restart.
3. Pick how everyone else signs in at **`/admin/directory`**.

The `admin` account always exists, works in every sign-in mode, and sees and
drives every machine. The name `admin` is reserved and never sent to the directory.

## Sign-in modes

| mode | who checks passwords | when it is used |
|---|---|---|
| `local` | the built-in `admin`, `HUB_USERS`, and accounts an admin creates at `/admin/users` | the default when no LDAP host is set |
| `ldap` | your LDAP or Active Directory, by binding as the user | the default when `HUB_LDAP_HOST` is set |
| `mockldap` | demo users kept in `hub.db`, hashed | only if you set `HUB_AUTH_MODE=mockldap` |

Local accounts sign in in every mode. The admin can switch the mode at
`/admin/directory`; it takes effect on the next sign-in, with no restart.
ADFS or another OpenID Connect provider can be added as a sign-in button
(`HUB_OIDC_*`, see `hub-go/API.md`).

## Users

The admin manages accounts at **`/admin/users`**: add a user, reset a password,
delete a user. `HUB_USERS` adds accounts from configuration:
`alice=<bcrypt or sha256:hex>[:admin]`, hashed with `klarkhub hash-password`.

The mock directory (`mockldap`, opt-in) seeds `user1`/`user1pass` and
`user2`/`user2pass`; `HUB_MOCK_USERS` seeds others. Use it for trials only.

## Who sees and who types

| setting | default | effect |
|---|---|---|
| `HUB_LDAP_ALLOWED_GROUPS` | empty (anyone) | only members of these groups may sign in |
| `HUB_LDAP_ADMIN_GROUP` | empty | members are admins and see every machine |

Per-machine access groups are not available yet in the current hub.

Sign-in failures are rate limited per user and per address.

## Tokens for the CLI

The CLI token page is not available yet in the current hub.
