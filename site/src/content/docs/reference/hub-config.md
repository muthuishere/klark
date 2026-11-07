---
title: Hub configuration
description: Every Hub:* setting, its environment variable, and its default.
---

The hub reads the `Hub` section from `appsettings.json`,
`appsettings.Production.json`, and environment variables with the **`KLARK_`**
prefix. Each `:` becomes `__`: `Hub:Auth:Mode` is `KLARK_HUB__AUTH__MODE`.
Command-line arguments win over the environment.

Source of truth: `hub/Services/HubOptions.cs`.

## General

| setting | env | default | meaning |
|---|---|---|---|
| `Hub:DataDir` | `KLARK_HUB__DATADIR` | `data` (`/data` in the image) | where `hub.db`, `machines/`, `assets/`, `keys/` and `jwt.key` live |
| `Hub:MaxBytesPerMachine` | `KLARK_HUB__MAXBYTESPERMACHINE` | `10485760` | cap on each machine's SQLite file; oldest output dropped first |
| `Hub:PublicUrl` | `KLARK_HUB__PUBLICURL` | empty | the address people open; used in `klark hub connect` links. Empty: taken from the request |
| `Hub:TrustForwardedHeaders` | `KLARK_HUB__TRUSTFORWARDEDHEADERS` | `false` | honour `X-Forwarded-For/Proto/Host` behind a proxy. Leave off otherwise, or clients can fake their address past the sign-in limit |
| `Hub:Admin:Password` | `KLARK_HUB__ADMIN__PASSWORD` | empty (`adminadmin`) | the built-in admin's password on first start only |

## Sign-in

| setting | env | default | meaning |
|---|---|---|---|
| `Hub:Auth:Mode` | `KLARK_HUB__AUTH__MODE` | empty | `Ldap`, `MockLdap` or `Local`. Empty: `Ldap` if an LDAP host is set, else `MockLdap`. `/admin/directory` overrides it |
| `Hub:Auth:AllowedGroups` | `KLARK_HUB__AUTH__ALLOWEDGROUPS__0` | `[]` | if set, only these groups may sign in |
| `Hub:Auth:AdminGroups` | `KLARK_HUB__AUTH__ADMINGROUPS__0` | `["klark-admins"]` | members see every machine |
| `Hub:Auth:ReadOnlyGroups` | `KLARK_HUB__AUTH__READONLYGROUPS__0` | `["klark-readonly"]` | members watch but never type |
| `Hub:Auth:MockUsers` | `KLARK_HUB__AUTH__MOCKUSERS__0__UID`, `…__PASSWORD`, `…__DISPLAYNAME`, `…__GROUPS__0` | `[]` (seeds `user1`, `user2`) | seed for the mock directory on first start |
| `Hub:Users` | `KLARK_HUB__USERS__0__NAME`, `…__PASSWORDHASH` | `[]` | users for `Local` mode; hash with `dotnet run -- hash-password <pw>` |

Lists use an index: `KLARK_HUB__AUTH__ALLOWEDGROUPS__0=klark-users`,
`KLARK_HUB__AUTH__ALLOWEDGROUPS__1=ops`.

## LDAP

All under `Hub:Auth:Ldap:` (env `KLARK_HUB__AUTH__LDAP__…`).

| setting | default |
|---|---|
| `Host` | `localhost` |
| `Port` | `0` (636 with `UseSsl`, else 389) |
| `UseSsl` | `false` |
| `StartTls` | `false` |
| `SkipCertificateCheck` | `false` |
| `UserDnTemplate` | `cn={0},ou=users,dc=klark,dc=local` |
| `UserSearchBase` | empty |
| `UserFilter` | `(\|(uid={0})(sAMAccountName={0})(cn={0}))` |
| `BindDn` | empty |
| `BindPassword` | empty (give it through the environment) |
| `GroupSearchBase` | `dc=klark,dc=local` |
| `UseMemberOf` | `true` |
| `TimeoutSeconds` | `10` |

What each one does: [LDAP and Active Directory](/hub/ldap/).

## Tokens

| setting | env | default | meaning |
|---|---|---|---|
| `Hub:Jwt:Key` | `KLARK_HUB__JWT__KEY` | empty | HS256 key, at least 32 bytes. Empty: a random key in `<DataDir>/jwt.key` (mode 0600) |
| `Hub:Jwt:Hours` | `KLARK_HUB__JWT__HOURS` | `12` | user token lifetime |

## Machines from configuration

Machines approved through `klark hub connect` are stored in `hub.db`. You can
also list machines here.

| setting | env | meaning |
|---|---|---|
| `Hub:Machines[].Name` | `KLARK_HUB__MACHINES__0__NAME` | machine name on the hub |
| `Hub:Machines[].TokenSha256` | `KLARK_HUB__MACHINES__0__TOKENSHA256` | hex SHA-256 of the token: `dotnet run -- hash-token <token>` |
| `Hub:Machines[].Fleet` | `KLARK_HUB__MACHINES__0__FLEET` | one token for many machines, named `<Name>:<X-Klark-Machine>` |
| `Hub:Machines[].Groups` | `KLARK_HUB__MACHINES__0__GROUPS__0` | only these groups see it; empty means everyone |

## Assets and files

| setting | env | default | meaning |
|---|---|---|---|
| `Hub:Assets:RetentionHours` | `KLARK_HUB__ASSETS__RETENTIONHOURS` | `24` | every asset is deleted after this |
| `Hub:Assets:MaxImageBytes` | `KLARK_HUB__ASSETS__MAXIMAGEBYTES` | `20971520` (20 MB) | image a person sends to an agent |
| `Hub:Assets:MaxFileBytes` | `KLARK_HUB__ASSETS__MAXFILEBYTES` | `104857600` (100 MB) | file an agent offers |
| `Hub:Files:MaxBytes` | `KLARK_HUB__FILES__MAXBYTES` | `10485760` (10 MB) | largest file the viewer fetches |

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
