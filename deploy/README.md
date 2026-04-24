# `deploy/`

Build artefacts for the explorer's two container images:

* `docker/Dockerfile.backend` — multi-stage Rust build, distroless runtime.
  One image, two roles: `--mode=server` (HTTP API) and `--mode=indexer`
  (no listener). The Deployment picks the role via `args:`.
* `docker/Dockerfile.web` — multi-stage Nuxt 4 build, Node runtime.

Images are published to GHCR by `.github/workflows/images.yml` on every
push to `main` and on `v*` tags.

**The Kubernetes manifests do not live here.** They live in the
`infra-web3` GitOps repo under:

```
infra-web3/kubernetes/base/apps/allfeat-explorer/
infra-web3/kubernetes/overlays/prod/apps/allfeat-explorer/
```

ArgoCD syncs them into the `allfeat-explorer-prod` namespace via the
`allfeat-explorer` Application (declared in `prod/apps/workloads.yaml`).
See [`docs/deployment.md`](../docs/deployment.md) for the full runbook.
