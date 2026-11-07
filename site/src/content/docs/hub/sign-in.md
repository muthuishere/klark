---
title: First sign-in and users
description: The built-in admin, the mock directory, groups, and who may see and type.
---

## Sign in as admin

1. Open the hub and sign in as **`admin`** / **`adminadmin`**. On Kubernetes,
   use the Secret's `admin-password` instead.
2. Change the password. The hub shows a banner while the default is in use. Or
   open **⋯ → Change password** (`/account/password`).
3. Pick how everyone else signs in at **⋯ → Sign-in directory**
   (`/admin/directory`).

The `admin` account always exists, works in every sign-in mode, and sees and
drives every machine. Its password hash lives in `hub.db`. The name `admin` is
reserved and never sent to the directory.

`KLARK_HUB__ADMIN__PASSWORD` sets the admin password only when `hub.db` has none
yet, so changing the variable later does nothing.

## Sign-in modes

| mode | who checks passwords | when it is used |
|---|---|---|
| `Ldap` | your LDAP or Active Directory, by binding as the user | the default when `Hub:Auth:Ldap:Host` is set |
| `MockLdap` | users kept in `hub.db`, hashed | the default when no LDAP host is set |
| `Local` | password hashes in `Hub:Users` in config | only if you set `Hub:Auth:Mode=Local` |

The admin can switch the mode at `/admin/directory`. It takes effect on the
next sign-in, with no restart.

## The mock directory

A fresh hub with no LDAP host seeds two users:

| user | password |
|---|---|
| `user1` | `user1pass` |
| `user2` | `user2pass` |

The admin manages them at **`/admin/users`**: add a user, reset a password,
delete a user. Users change their own password at `/account/password`.

To seed different users on first start, set `Hub:Auth:MockUsers` (each entry has
`Uid`, `Password`, `DisplayName`, `Groups`). Use the mock directory for trials;
use LDAP for real teams.

## Who sees and who types

Access follows groups. They come from the directory (LDAP) or from the mock
user's `Groups`.

| setting | default | effect |
|---|---|---|
| `Hub:Auth:AllowedGroups` | empty (anyone) | only members of these groups may sign in |
| `Hub:Auth:AdminGroups` | `klark-admins` | members see every machine |
| `Hub:Auth:ReadOnlyGroups` | `klark-readonly` | members watch but never type (enforced on the server) |
| `Hub:Machines[].Groups` | empty (everyone) | only these groups see that configured machine |

Sign-in is limited to 10 attempts per minute per address.

## Tokens for the CLI

A signed-in user opens **CLI token** (`/token`) for a JWT with a copy button.
Scripts can ask for one directly:

```sh
curl -s -X POST http://localhost:8080/api/token \
  -H 'content-type: application/json' \
  -d '{"username":"user1","password":"user1pass"}'
# {"token":"eyJ…","expires_at":"…","user":"user1","groups":["klark-users"]}
```

Tokens last 12 hours (`Hub:Jwt:Hours`). Deleting `/data/jwt.key` revokes every
token, unless you set the key yourself with `KLARK_HUB__JWT__KEY`.
