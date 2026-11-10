---
title: Connect machines
description: klark hub connect links a machine with a browser approval. Saved hubs, reconnect, revoke.
---

A machine joins a hub with a device code. Nobody copies a token by hand.

## Connect

On the machine:

```sh
klark hub connect --url http://hub.lan:7470
```

It prints a code such as `ABCD-EFGH` and a link, and opens your browser
(`--no-browser` to skip that). Open the link while signed in to the hub, check
the code matches, and press **Approve**. The machine receives its token and saves it.

Then the connection **runs in the background**, detached from your terminal. Closing the
tab does not drop the machine off the hub. It logs to `~/.config/klark/hub-<name>.log`.
Add `--foreground` to keep it in the terminal instead (`ctrl+c` ends it).

- **The hub becomes the default hub.** The first hub you save is the default; `klark msg` and
  `klark offer` use it.
- **One hub connection at a time.** Connecting to a hub ends any other background connection.
- Codes expire after 10 minutes. Any signed-in user may approve. They are recorded
  as the machine's owner, and only they or an admin can re-key a machine of that name.

```sh
klark hub stop        # end the connection (alias: disconnect)
klark hub ls          # saved hubs; tokens are never printed
klark hub status      # URL, reachable?, machine name, is a link running?
```

To check it worked, open the hub: the machine's agents appear in the sidebar.

## Saved hubs

Hubs are saved in `~/.config/klark/hubs.json` (mode 0600). The name defaults to
the URL's host; `--as <name>` picks another.

```sh
klark hub connect                     # reconnect to the default hub with the saved token
klark hub connect lab                 # a saved hub by name
klark hub connect lab --agents reviewer,builder   # expose only some agents
klark hub ls                          # saved hubs
klark hub use lab                     # make "lab" the default
klark hub status [lab]
klark hub rm lab
```

If the hub answers 401 or 403 for a saved token, `connect` runs the browser
approval again.

`klark msg` and `klark offer` use the default hub when `KLARK_HUB_URL` and
`KLARK_HUB_TOKEN` are not set.

## Scripts and CI

Skip the browser with a machine token:

```sh
klark hub connect --url http://hub.lan:7470 --token-stdin < token.txt
KLARK_HUB_TOKEN=… klark hub connect --url http://hub.lan:7470
```

## HTTPS and private CAs

`https://` hubs work (rustls with the standard web roots). For a private or
self-signed CA:

```sh
KLARK_HUB_CA_FILE=/path/ca.pem klark hub connect --url https://klark.corp.example.com
```

## List and revoke

Admins see every machine at **`/admin/machines`**. **Revoke** drops the live
link at once; the token then gets 401. The machine has to be approved again.

## Machines from configuration

You can also list machines in `Hub:Machines` with the SHA-256 of their token,
which is useful for fleets and load tests:

```sh
cd hub
dotnet run -- hash-token "$(openssl rand -hex 32)"    # prints TokenSha256; keep the token
```

A `Fleet: true` entry lets one token serve many machines; each connection is
named `<Name>:<X-Klark-Machine header>`. `Groups` limits who sees the machine.
Both approved and configured tokens are accepted.
