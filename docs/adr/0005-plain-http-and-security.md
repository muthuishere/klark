# ADR 0005: plain HTTP

- Status: accepted (decided by the owner)

## Decision
The hub serves plain HTTP on :8080, with no HSTS and no HTTPS redirect.
Cookies follow the request scheme. Machines use `ws://`.

## Why not HLS
HLS is segmented video: it adds 2–10 s of latency, only goes one way, and would
mean rendering text to video. Terminal output is small text, so WebSocket
(machines) and SignalR (browsers) are faster and two-way. (Also in ADR 0001 D9.)

## Consequences
- Passwords, session cookies and machine tokens cross the network in clear
  text. Run the hub on a trusted LAN, VPN or tailnet.
- Moving to TLS later needs no code change: a TLS proxy in front, `wss://`
  in `klark link` (supported), and cookies become Secure automatically under
  HTTPS.
- Things that still hold over HTTP:
  - the hub can only read panes, type and send keys (allow-list, ADR 0002);
  - antiforgery tokens on forms;
  - login redirects are local-only.
