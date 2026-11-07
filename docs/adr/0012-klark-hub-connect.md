# ADR 0012: `klark hub connect` — link a machine to a hub with a device code

- Status: accepted (owner decision, 2026-11-07). Builds on ADR 0010 (`klark expose`).

## Decision
The hub (klarkhub, the `hub/` image) is a separate server the owner deploys
anywhere (Kubernetes, Docker). The `klark` binary carries only the client side:

1. Deploy klarkhub.
2. `klark hub connect --url http(s)://host[:port]` on the machine. No token is
   typed: klark runs a device-authorization flow against the hub:
   - `POST /api/connect/start {"name": <machine>}` → `{code, poll_id, url, expires_in, interval}`
   - klark prints "Open <url> and sign in to approve this machine (code ABCD-EFGH)"
     and opens the browser (`--no-browser` to skip);
   - `POST /api/connect/poll {"poll_id"}` every `interval` s →
     `pending | denied | expired` or `{status: "approved", token, machine}`.
3. The token is saved, then klark runs `klark expose` in-process against
   `ws(s)://host[:port]/agent`.

Saved hubs: `<config dir>/hubs.json`, mode 0600,
`{"default": name, "hubs": {name: {url, token, machine}}}`. The name defaults to
the URL's host (`--as` overrides); the first hub saved is the default.
`klark hub connect [<name>]` reconnects to a saved hub (default when omitted);
if the hub answers 401/403 for the saved token, the device flow runs again.
`klark hub ls | use | rm | status` manage the list; tokens are never printed.

`--token`, `--token-stdin` and `KLARK_HUB_TOKEN` remain for scripts.
`klark msg` and `klark offer` fall back to the default hub (URL and machine
token) when `KLARK_HUB_URL` / `KLARK_HUB_TOKEN` are not set; a `klark msg
login` JWT in hub.json is used only for the hub it was issued by.

## TLS
`https://` hubs work end to end (ADR 0005 update): rustls with webpki roots,
plus `KLARK_HUB_CA_FILE` (PEM) for a private ingress CA. Plain http keeps working.

## Limits
- The saved-token check is a `GET /agent` with the bearer token; only 401/403
  count as rejection.
