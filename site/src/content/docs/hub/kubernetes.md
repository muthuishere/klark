---
title: Kubernetes
description: A walkthrough of deploy/k8s/klarkhub.yaml, a single-replica Deployment with a PVC and a NodePort.
---

`deploy/k8s/klarkhub.yaml` is a plain example with three objects. No Ingress,
no TLS, no WebSockets.

## 1. Build and push the image

```sh
docker build -t <registry>/klarkhub:latest hub
docker push <registry>/klarkhub:latest
```

The manifest uses `image: klarkhub:latest`. Change it to your registry's name.

## 2. Create the Secret

```sh
kubectl create secret generic klarkhub \
  --from-literal=admin-password="$(openssl rand -base64 18)" \
  --from-literal=jwt-key="$(openssl rand -base64 48)"
```

Both keys are optional. `admin-password` sets the built-in admin's password on
the **first** start only. `jwt-key` signs user tokens; without it the hub
creates a key in `/data/jwt.key`.

## 3. Apply

```sh
kubectl apply -f deploy/k8s/
kubectl get pods -l app=klarkhub
```

Open `http://<node-ip>:30080`.

## What is in the manifest

**PersistentVolumeClaim `klarkhub-data`**: 10 Gi, `ReadWriteOnce`, mounted at
`/data`.

**Deployment `klarkhub`**:

- `replicas: 1` and `strategy: Recreate`. Never two pods on one SQLite file.
- `fsGroup: 1654`, the .NET image's `app` user, so `/data` is writable.
- Env `KLARK_HUB__ADMIN__PASSWORD` and `KLARK_HUB__JWT__KEY` from the Secret.
- Readiness and liveness probes on `GET /healthz`.
- Resources from E-06: requests `250m` CPU / `512Mi`, limits `1` CPU / `512Mi`.
  Raise memory to `1Gi` for about 100 machines.

**Service `klarkhub`**: `NodePort` 30080 → 7470. Use `LoadBalancer` if your
cluster has one.

## Set the public URL

`klark hub connect` prints a link for people to open. Tell the hub its real
address so that link is right. Uncomment this in the manifest:

```yaml
- name: KLARK_HUB__PUBLICURL
  value: http://<node-ip>:30080
```

## Behind an ingress or proxy

Set `KLARK_HUB__TRUSTFORWARDEDHEADERS=true` so the hub reads `X-Forwarded-*`.
Turn off response buffering and allow responses to stay open 60 s or more. The
hub already sends `X-Accel-Buffering: no` on the machine event stream, which
nginx honours.
