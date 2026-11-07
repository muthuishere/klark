---
title: LDAP and Active Directory
description: Point the hub at your directory, test the connection, then switch.
---

In `Ldap` mode your directory checks every password by a bind as the user. The
hub stores no directory passwords.

## In the UI (no restart)

1. Sign in as `admin` and open **⋯ → Sign-in directory** (`/admin/directory`).
2. Fill in the LDAP settings and save. Saved settings live in `hub.db` and
   override configuration for the fields you save.
3. Use **Test connection**: type a real user name and password. The hub binds
   with them against the saved settings and reports the result.
4. Switch the mode to `Ldap`. It applies from the next sign-in.

The `admin` account keeps working, so a wrong setting never locks you out.

## By configuration

Set the same fields as environment variables (or `hub/appsettings.Production.json`).
Setting `KLARK_HUB__AUTH__LDAP__HOST` alone makes `Ldap` the default mode.

### Active Directory, direct bind with the UPN

```sh
docker run -d --name klarkhub -p 8080:8080 -v klarkhub-data:/data \
  -e KLARK_HUB__AUTH__LDAP__HOST=dc1.corp.example.com \
  -e KLARK_HUB__AUTH__LDAP__USESSL=true \
  -e 'KLARK_HUB__AUTH__LDAP__USERDNTEMPLATE={0}@corp.example.com' \
  -e KLARK_HUB__AUTH__LDAP__GROUPSEARCHBASE= \
  -e KLARK_HUB__AUTH__LDAP__USEMEMBEROF=true \
  klarkhub
```

### OpenLDAP, search then bind

```sh
-e KLARK_HUB__AUTH__LDAP__HOST=ldap.example.com
-e KLARK_HUB__AUTH__LDAP__STARTTLS=true
-e KLARK_HUB__AUTH__LDAP__USERSEARCHBASE=ou=people,dc=example,dc=com
-e 'KLARK_HUB__AUTH__LDAP__USERFILTER=(uid={0})'
-e KLARK_HUB__AUTH__LDAP__BINDDN=cn=klark-svc,ou=services,dc=example,dc=com
-e KLARK_HUB__AUTH__LDAP__BINDPASSWORD=…        # from a secret, never a file in git
-e KLARK_HUB__AUTH__LDAP__GROUPSEARCHBASE=ou=groups,dc=example,dc=com
```

## Settings

| setting | default | meaning |
|---|---|---|
| `Host` | `localhost` | LDAP server |
| `Port` | `0` | 0 picks 636 with `UseSsl`, else 389 |
| `UseSsl` | `false` | LDAPS, TLS from the first byte |
| `StartTls` | `false` | upgrade a plain connection before binding |
| `SkipCertificateCheck` | `false` | accept any certificate (test servers only) |
| `UserDnTemplate` | `cn={0},ou=users,dc=klark,dc=local` | direct bind DN or UPN; `{0}` is the escaped user name. Ignored when `UserSearchBase` is set |
| `UserSearchBase` | empty | when set: search for the user here, then bind as the DN found |
| `UserFilter` | `(\|(uid={0})(sAMAccountName={0})(cn={0}))` | search filter |
| `BindDn` / `BindPassword` | empty | service account for the search (anonymous if empty) |
| `GroupSearchBase` | `dc=klark,dc=local` | where groups live; members matched by `member`, `uniqueMember` or `memberUid`. Empty skips the search |
| `UseMemberOf` | `true` | also read the user's `memberOf` |
| `TimeoutSeconds` | `10` | connection timeout |

Every setting is under `Hub:Auth:Ldap:`, as env `KLARK_HUB__AUTH__LDAP__<NAME>`.

## Try it without a directory

`hub/docker-compose.yml` has an `ldap` profile with a throwaway OpenLDAP
(users `alice`/`alice` and `bob`/`bob`, group `readers`):

```sh
cd hub
echo '{}' > appsettings.Production.json
docker compose --profile ldap up -d --build
```

If the `bitnami/openldap:2.6` image no longer pulls, use
`bitnamilegacy/openldap:2.6` or another OpenLDAP image.

:::caution
Status: `Ldap` mode builds and is covered by code review, but sign-in against a
real directory has not been tried yet (STATUS S-28). Test with **Test
connection** before you switch.
:::
