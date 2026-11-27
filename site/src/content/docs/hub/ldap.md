---
title: LDAP and Active Directory
description: Point the hub at your directory, test the connection, then switch.
---

In `ldap` mode your directory checks every password by a bind as the user. The
hub stores no directory passwords.

## In the UI (no restart)

1. Sign in as `admin` and open **`/admin/directory`**.
2. Fill in the LDAP settings and save. Saved settings live in `hub.db` and
   override configuration for the fields you save.
3. Use **Test connection**: type a real user name and password. The hub binds
   with them against the saved settings and reports the result.
4. Switch the mode to `ldap`. It applies from the next sign-in.

The `admin` account keeps working, so a wrong setting never locks you out.

## By configuration

Set the same fields as environment variables. Setting `HUB_LDAP_HOST` alone
makes `ldap` the default mode. Source of truth: `hub-go/API.md`.

### Active Directory, direct bind with the UPN

```sh
docker run -d --name klarkhub -p 7470:7470 -v klarkhub-data:/data \
  -e HUB_LDAP_HOST=dc1.corp.example.com \
  -e HUB_LDAP_SSL=true \
  -e 'HUB_LDAP_USER_DN_TEMPLATE={0}@corp.example.com' \
  -e HUB_LDAP_USE_MEMBEROF=true \
  ghcr.io/muthuishere/klarkhub:latest
```

### OpenLDAP, search then bind

```sh
-e HUB_LDAP_HOST=ldap.example.com
-e HUB_LDAP_STARTTLS=true
-e HUB_LDAP_USER_SEARCH_BASE=ou=people,dc=example,dc=com
-e 'HUB_LDAP_USER_FILTER=(uid={0})'
-e HUB_LDAP_BIND_DN=cn=klark-svc,ou=services,dc=example,dc=com
-e HUB_LDAP_BIND_PASSWORD=…        # from a secret, never a file in git
-e HUB_LDAP_GROUP_SEARCH_BASE=ou=groups,dc=example,dc=com
```

## Settings

| variable | default | meaning |
|---|---|---|
| `HUB_LDAP_HOST` | empty | LDAP server |
| `HUB_LDAP_PORT` | 389, 636 with SSL | port |
| `HUB_LDAP_SSL` | `false` | LDAPS, TLS from the first byte |
| `HUB_LDAP_STARTTLS` | `false` | upgrade a plain connection before binding |
| `HUB_LDAP_SKIP_CERT_CHECK` | `false` | accept any certificate (test servers only) |
| `HUB_LDAP_USER_DN_TEMPLATE` | empty | direct bind DN or UPN; `{0}` is the user name |
| `HUB_LDAP_USER_SEARCH_BASE`, `_USER_FILTER` | empty, `(uid={0})` | search for the user, then bind as the DN found (wins over the template) |
| `HUB_LDAP_BIND_DN`, `_BIND_PASSWORD` | empty | service account for searches (anonymous if empty) |
| `HUB_LDAP_GROUP_SEARCH_BASE`, `_GROUP_FILTER` | empty, `(\|(member={0})(uniqueMember={0})(memberUid={1}))` | where groups live |
| `HUB_LDAP_USE_MEMBEROF` | `false` | also read the user's `memberOf` |
| `HUB_LDAP_DISPLAY_NAME_ATTR` | `displayName`, then `cn` | display name attribute |
| `HUB_LDAP_ADMIN_GROUP`, `_ALLOWED_GROUPS` | empty | admins; groups allowed to sign in (empty = everyone) |
| `HUB_LDAP_TIMEOUT_SECONDS` | `10` | connection timeout |

## Try it without a directory

`HUB_AUTH_MODE=mockldap` turns on a mock directory with demo users (see
[First sign-in](/hub/sign-in/)). It checks passwords in `hub.db`, not over LDAP.

:::caution
Status: sign-in against a real directory has not been tried yet (STATUS S-28). Test with **Test
connection** before you switch.
:::
