# Deploying the hub (`klarkhub`)

The hub is one container, image `klarkhub`, serving plain HTTP on port 7470
with all state in `/data` (`hub.db`, `session.key`, backups). Run exactly
one replica: SQLite is a local file.

## Docker

```sh
docker build -t klarkhub hub-go
docker run -d --name klarkhub -p 7470:7470 -v klarkhub-data:/data klarkhub
```

With nothing configured only the built-in `admin` (`adminadmin`, or
`HUB_ADMIN_PASSWORD`) and `HUB_USERS` sign in. Add
`-e HUB_LDAP_HOST=ldap.corp.example.com` (and the other `HUB_LDAP_*` settings,
see `hub-go/API.md`) to use LDAP, or switch later in the UI at `/admin/directory`.

## Kubernetes

```sh
kubectl create secret generic klarkhub \
  --from-literal=admin-password="$(openssl rand -base64 18)" \
  --from-literal=jwt-key="$(openssl rand -base64 48)"   # optional
kubectl apply -f deploy/k8s/
```

The example manifest still passes these as the old hub's `KLARK_HUB__ADMIN__PASSWORD` and
`KLARK_HUB__JWT__KEY`, which the current hub does not read: until it is updated, rename the first to
`HUB_ADMIN_PASSWORD` (the second has no equivalent), or admin keeps `adminadmin`.

`deploy/k8s/` is a plain example: a Deployment (1 replica, `Recreate`), a
NodePort Service (plain http on every node at `:30080`), a PVC for `/data`, and
probes on `/healthz`. Open `http://<node-ip>:30080`. Set `HUB_PUBLIC_URL`
to that address so the links `klark hub connect` prints point there.

No Ingress or TLS is needed, and nothing uses WebSockets (ADR 0013): machines
and browsers speak ordinary HTTP requests, so any proxy, NodePort or corporate
firewall that passes HTTP passes the hub. Behind a proxy, two settings help:
do not buffer responses (the hub already sends `X-Accel-Buffering: no` on the
machine event stream, which nginx honours) and allow a response to stay open
for at least 60 s (`/agent/events` is held open with a line every 15 s; the
browser's live views are Server-Sent Events). Behind a proxy the hub takes its
public address from `X-Forwarded-*` unless `HUB_PUBLIC_URL` is set.

## First sign-in

1. Sign in as `admin` with `adminadmin` (or `HUB_ADMIN_PASSWORD`).
2. The current hub has no change-password page yet: set the password with
   `HUB_ADMIN_PASSWORD`.
3. Pick the sign-in directory at `/admin/directory`:
   save LDAP settings, use Test connection, then switch the mode. Or manage the
   mock directory's users at `/admin/users`.
4. Connect a machine: on it, run `klark hub connect --url http://<node-ip>:30080`.
   It prints a code and a link; open the link signed in, check the code, and
   Approve. The machine receives its token by itself; nothing is copied by hand.
   Admins list and revoke machines at `/admin/machines`.
