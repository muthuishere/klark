# ADR 0004: authentication: people and machines

- Status: accepted, implemented (`hub/Services/Directory.cs`, `Program.cs`)

## People
- `Hub:Auth:Mode`:
  - **MockLdap** (default): an in-memory directory with demo users admin,
    alice and bob (the password equals the name). It is for development and
    demos; a warning is logged at startup.
  - **Ldap**: simple bind as `UserDnTemplate` (user name escaped), then a group
    search on `member`, `uniqueMember` or `memberUid`.
  - **Local**: `Hub:Users` with ASP.NET `PasswordHasher` hashes
    (`dotnet run -- hash-password`).
- `Hub:Auth:AllowedGroups` lets in only members of the listed groups.
- Cookie session for 14 days, sliding, HttpOnly, SameSite=Lax.
- Groups are stored as role claims.

## Machines
- Each `Hub:Machines` entry has `Name` and `TokenSha256`
  (`dotnet run -- hash-token`). Tokens are compared in constant time.
- The machine's name on the hub comes from config, never from what the machine
  claims about itself.
- `Fleet: true`: one token admits many machines, named
  `Name:<X-Klark-Machine>` (sanitized, at most 64 characters). Used by the
  load test.
- A second connection under the same name replaces the first.

## Visibility and permissions (implemented in backlog round 1)
- `Hub:Machines[].Groups`: a machine is visible to members of these groups, or
  to everyone when the list is empty. Fleet members inherit their entry's
  groups. (S-05)
- `Hub:Auth:AdminGroups` (default `klark-admins`) see every machine.
- `Hub:Auth:ReadOnlyGroups` (default `klark-readonly`) can watch but not type.
  `HubAccess.CanType` is checked inside the server-side handlers, not only in
  the UI. (S-06)
- Sign-in is rate-limited to 10 attempts per minute per address. (S-11)

## Not decided / not done
- An admin UI for tokens and users stored in hub.db. (S-08)
