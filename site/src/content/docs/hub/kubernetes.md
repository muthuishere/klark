---
title: Kubernetes
description: A walkthrough of deploy/k8s/klarkhub.yaml, a single-replica Deployment with a PVC and a NodePort.
---

`deploy/k8s/klarkhub.yaml` is a plain example with three objects. No Ingress,
no TLS, no WebSockets.

## 1. Build and push the image

```sh
docker build -t <registry>/klarkhub:latest hub-go
docker push <registry>/klarkhub:latest
```

The manifest uses `image: klarkhub:latest`. Change it to your registry's name.

## 2. Create the Secret

```sh
kubectl create secret generic klarkhub \
  --from-literal=admin-password="$(openssl rand -base64 18)"
```

`admin-password` is the built-in admin's password (`HUB_ADMIN_PASSWORD`).

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
- `fsGroup: 1654`, so `/data` is writable (the image runs as non-root `65532`).
- Env from the Secret (see the caution above).
- Readiness and liveness probes on `GET /healthz`.
- Resources from E-06 (measured on the old Blazor hub): requests `250m` CPU / `512Mi`, limits `1` CPU / `512Mi`.
  Raise memory to `1Gi` for about 100 machines.

**Service `klarkhub`**: `NodePort` 30080 → 7470. Use `LoadBalancer` if your
cluster has one.

## Set the public URL

`klark hub connect` prints a link for people to open. Tell the hub its real
address so that link is right. In the manifest:

```yaml
- name: HUB_PUBLIC_URL
  value: http://<node-ip>:30080
```

## Behind an ingress or proxy

The hub reads `X-Forwarded-*` for its public address unless `HUB_PUBLIC_URL` is set.
Turn off response buffering and allow responses to stay open 60 s or more. The
hub already sends `X-Accel-Buffering: no` on the machine event stream, which
nginx honours.
