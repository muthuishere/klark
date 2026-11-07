# Deploying the hub (`klarkhub`)

The hub is one container, image `klarkhub`, serving plain HTTP on port 8080
with all state in `/data` (hub.db, machine files, assets, keys). Run exactly
one replica: SQLite and the local asset store are not shared.

## Docker

```sh
docker build -t klarkhub hub
docker run -d --name klarkhub -p 8080:8080 -v klarkhub-data:/data klarkhub
```

With nothing configured the hub signs people in with its built-in mock
directory (users `user1`/`user1pass`, `user2`/`user2pass`). Add
`-e KLARK_HUB__AUTH__LDAP__HOST=ldap.corp.example.com` (and the other
`KLARK_HUB__AUTH__LDAP__*` settings) to use LDAP instead, or switch later in the UI.

## Kubernetes

```sh
kubectl create secret generic klarkhub \
  --from-literal=admin-password="$(openssl rand -base64 18)" \
  --from-literal=jwt-key="$(openssl rand -base64 48)"   # optional
kubectl apply -f deploy/k8s/
```

`deploy/k8s/` is a plain example: a Deployment (1 replica, `Recreate`), a
NodePort Service (plain http on every node at `:30080`), a PVC for `/data`, and
probes on `/healthz`. Open `http://<node-ip>:30080`. Set `KLARK_HUB__PUBLICURL`
to that address so the links `klark hub connect` prints point there.

No Ingress or TLS is needed, and nothing uses WebSockets (ADR 0013): machines
and browsers speak ordinary HTTP requests, so any proxy, NodePort or corporate
firewall that passes HTTP passes the hub. Behind a proxy, two settings help:
do not buffer responses (the hub already sends `X-Accel-Buffering: no` on the
machine event stream, which nginx honours) and allow a response to stay open
for at least 60 s (`/agent/events` is held open with a line every 15 s; the
browser's `/_blazor` uses Server-Sent Events or long polling). Set
`KLARK_HUB__TRUSTFORWARDEDHEADERS=true` there.

## First sign-in

1. Sign in as `admin` with `adminadmin` (or the Secret's `admin-password`).
2. Change the password: the banner, or ⋯ → Change password (`/account/password`).
3. Pick the sign-in directory at ⋯ → Sign-in directory (`/admin/directory`):
   save LDAP settings, use Test connection, then switch the mode. Or manage the
   mock directory's users at `/admin/users`.
4. Connect a machine: on it, run `klark hub connect --url http://<node-ip>:30080`.
   It prints a code and a link; open the link signed in, check the code, and
   Approve. The machine receives its token by itself; nothing is copied by hand.
   Admins list and revoke machines at `/admin/machines`.
