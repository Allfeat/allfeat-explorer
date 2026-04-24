# Deployment guide

The Allfeat Explorer runs on the Allfeat Kubernetes cluster (`infra-web3`
repo) behind the nginx Gateway API, rolled out by ArgoCD. The canonical
manifests live in:

```
infra-web3/kubernetes/base/apps/allfeat-explorer/
infra-web3/kubernetes/overlays/prod/apps/allfeat-explorer/
```

This document covers the end-to-end flow from a code change in this
repo to a live pod on `https://explorer.allfeat.org`. For day-to-day
operations once the stack is deployed (backfill, backups, reconciliation,
runtime config) see [`ops.md`](ops.md).

## Architecture

```
                       explorer.allfeat.org  (TLS terminated by gateway)
                                  │
            ┌─────────────────────┴─────────────────────┐
            │ HTTPRoute: /api/v1/*  →  allfeat-explorer-api  (:8088)
            │ HTTPRoute: /         →  allfeat-explorer-web  (:3000)
            └─────────────────────┬─────────────────────┘
                                  │
              ┌───────────────────┼───────────────────┐
              │                   │                   │
       allfeat-explorer-web   allfeat-explorer-api   allfeat-explorer-indexer
        (Nuxt 4 SSR, N ≥ 2)    (Rust, --mode=server, N ≥ 2)    (Rust, --mode=indexer, 1)
                                  │                   │
                                  └─────────┬─────────┘
                                            ▼
                                 allfeat-explorer-pg  (StatefulSet, 1, PVC)

RPC egress:  indexer + api  →  allfeat-rpc-archive.allfeat-rpc.svc.cluster.local:9944
```

Three container images are built from this repo:

| Component | Dockerfile                           | Image                                         |
|-----------|--------------------------------------|-----------------------------------------------|
| Backend   | `deploy/docker/Dockerfile.backend`   | `ghcr.io/allfeat/allfeat-explorer-backend`    |
| Frontend  | `deploy/docker/Dockerfile.web`       | `ghcr.io/allfeat/allfeat-explorer-web`        |
| Postgres  | _(upstream)_                         | `postgres:18-alpine`                          |

The backend image is mode-agnostic — the same binary serves the API
pod (`--mode=server`) and the indexer pod (`--mode=indexer`). The mode
flag lives in the Deployment spec, not the image.

## CI-driven image publishing

`.github/workflows/images.yml` builds and pushes both images to GHCR on:

* every push to `main` / `master` → `:main`, `:sha-<short>`
* every tag matching `v*`         → `:<version>`, `:<major>.<minor>`, `:latest`

The `sha-<short>` tag is the stable pin the GitOps repo bumps to. The
rolling tags (`main`, `latest`) exist for convenience — ArgoCD should
always sync on `sha-*` so rollouts are reproducible.

## First-time deploy

Prerequisites (already installed on the prod cluster):

* nginx Gateway Fabric (`main-gateway` in `gateway-system`)
* cert-manager + `letsencrypt-prod` ClusterIssuer
* sealed-secrets controller (`kube-system/sealed-secrets`)
* kube-prometheus-stack (ServiceMonitor CRD)
* Velero (PVC backups — picks up the `backup.velero.io/backup-volumes`
  annotation on the Postgres PVC template)
* An in-cluster Allfeat archive node at
  `allfeat-rpc-archive.allfeat-rpc.svc.cluster.local:9944`

Bootstrap steps, in order:

### 1. Add the public hostname to the gateway certificate

`explorer.allfeat.org` is already included in
`kubernetes/overlays/prod/platform/nginx-gateway/certificate.yaml`.
cert-manager issues the SAN once the DNS record points at the gateway
LoadBalancer. Confirm with:

```sh
kubectl -n gateway-system get certificate prod-multi-domain-tls
```

### 2. Generate the Postgres SealedSecret

The overlay ships a `db-sealedsecret.yaml` with placeholder values.
Before the first ArgoCD sync, replace it with a real sealed blob:

```sh
# Generate a strong password, then seal the secret against the
# cluster's sealed-secrets controller. `kubeseal` talks to the
# controller directly; you need kubectl context on the cluster.
PG_PW="$(openssl rand -base64 36 | tr -dc 'A-Za-z0-9' | head -c 48)"
PG_USER=explorer
PG_DB=explorer
DB_URL="postgres://${PG_USER}:${PG_PW}@allfeat-explorer-pg:5432/${PG_DB}?sslmode=disable"

kubectl -n allfeat-explorer-prod create secret generic allfeat-explorer-db \
  --from-literal=username="$PG_USER" \
  --from-literal=password="$PG_PW" \
  --from-literal=database-url="$DB_URL" \
  --dry-run=client -o yaml \
| kubeseal --format=yaml \
           --controller-namespace=kube-system \
           --controller-name=sealed-secrets \
> kubernetes/overlays/prod/apps/allfeat-explorer/db-sealedsecret.yaml
```

Commit the regenerated file to `infra-web3`.

### 3. Sync the ArgoCD Application

The `allfeat-explorer` Application is declared in
`kubernetes/overlays/prod/apps/workloads.yaml` (sync-wave 6). Merging
the infra-web3 PR makes the app-of-apps pick it up automatically; to
force an immediate sync:

```sh
argocd app sync allfeat-explorer
argocd app wait allfeat-explorer --health
```

First-time boot order inside the Application:

1. Namespace (`allfeat-explorer-prod`, sync-wave -1)
2. Postgres StatefulSet + SealedSecret
3. Indexer + API Deployments come up once `allfeat-explorer-db` exists;
   the backend's in-process migrator runs before the HTTP listener binds
4. Nuxt web Deployment
5. HTTPRoute (gateway attachment) — explorer.allfeat.org serves traffic

Expect the first backfill to take several hours. Watch it via the Grafana
metrics shipped by the ServiceMonitor (`indexer_lag_blocks` /
`indexer_backfill_remaining_blocks`) or directly in Postgres:

```sh
kubectl -n allfeat-explorer-prod exec -it statefulset/allfeat-explorer-pg -- \
    psql -U explorer -d explorer -c "SELECT * FROM indexer_cursor;"
```

## Rolling a new release

1. Merge a PR into `main` on the explorer repo. CI publishes the new
   images to GHCR (`main` + `sha-<short>`).
2. In `infra-web3`, bump the image tags in
   `kubernetes/overlays/prod/apps/allfeat-explorer/kustomization.yaml`:
   ```yaml
   images:
     - name: IMAGE_BACKEND
       newName: ghcr.io/allfeat/allfeat-explorer-backend
       newTag: sha-abcdef1     # paste the short SHA from CI
     - name: IMAGE_WEB
       newName: ghcr.io/allfeat/allfeat-explorer-web
       newTag: sha-abcdef1
   ```
3. Commit to `master` on infra-web3. ArgoCD's auto-sync picks the change
   up within a minute; a rolling restart replaces api + web pods with
   zero downtime (`maxSurge=1 maxUnavailable=0`). The indexer pod is
   `Recreate` — it flaps for a few seconds, which is fine.

### Rollback

Revert the infra-web3 commit. ArgoCD re-pins to the previous image tag.
Same zero-downtime properties apply.

## Configuration surface

Runtime config is split between the ConfigMap and the Secret:

| Key                          | Where                   | Notes                                    |
|------------------------------|-------------------------|------------------------------------------|
| `LISTEN_ADDR`                | ConfigMap               | `0.0.0.0:8088` inside the pod            |
| `RUST_LOG`                   | ConfigMap               | default `info,allfeat_explorer=info`     |
| `INDEXER_BACKFILL_CONCURRENCY` | ConfigMap             | default `8`                              |
| `WS_ALLOWED_ORIGINS`         | ConfigMap               | `https://explorer.allfeat.org`           |
| `API_ALLOWED_ORIGINS`        | ConfigMap               | `https://explorer.allfeat.org`           |
| `RPC_ENDPOINT_ALLFEAT`       | ConfigMap               | ws:// to the in-cluster archive node     |
| `DATABASE_URL`               | SealedSecret (Secret)   | assembled once at bootstrap              |
| `NUXT_API_ORIGIN`            | web Deployment env      | in-cluster URL of the API Service        |
| `NUXT_PUBLIC_WS_BASE`        | web Deployment env      | `wss://explorer.allfeat.org/api/v1`      |

A config change that requires a pod restart (anything in the ConfigMap):
bump a dummy annotation in the overlay to force a rollout, or `kubectl
rollout restart deploy -n allfeat-explorer-prod` — the ConfigMap is
mounted via `envFrom`, so pods only pick up changes on restart.

## Observability

* **Metrics** — `ServiceMonitor allfeat-explorer-api` scrapes `/metrics`
  at 30 s. Key series documented in [`ops.md`](ops.md).
* **Logs** — Alloy → Loki (default platform stack); query in Grafana
  with `{namespace="allfeat-explorer-prod"}`.
* **Health** — `/healthz` (liveness) and `/readyz` (readiness) on the
  API pods. The indexer pod has no HTTP surface; observe it through
  `indexer_cursor.updated_at` and log lines.

## Known limitations

* **Single Postgres writer.** The StatefulSet runs one replica. Data is
  persisted on a `ReadWriteOnce` PVC and backed up by Velero; if you
  need replication or streaming failover, swap the StatefulSet for a
  CloudNativePG Cluster in a follow-up.
* **Dev exposure not included.** Only the `prod` overlay exists. A
  `dev` overlay pointing at Melodie can be added later by duplicating
  `overlays/prod/apps/allfeat-explorer` under `overlays/dev/`.
