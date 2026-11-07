---
title: Kubernetes
description: Understand and adapt the Kubernetes manifest that ships in deploy/kubernetes.yaml.
sidebar:
  order: 3
---

This page walks through `deploy/kubernetes.yaml` so that you can adapt it to your cluster. Treat it as a starting point: it has been run on minikube with an S3 gateway. It has not been run on a production cluster or against real AWS.

## Before you apply it

The manifest has three replicas on one S3-compatible bucket, which is the only thing they share. [Many containers](/docs/operate/many-containers/) explains how that behaves.

Create the secret:

```bash
kubectl create secret generic rusticdoc \
  --from-literal=api-keys=... --from-literal=admin-keys=... \
  --from-literal=webhook-secret=... \
  --from-literal=s3-access-key=... --from-literal=s3-secret-key=...
```

Then change these in the file:

| Field | Change it to |
|---|---|
| `image` | The manifest says `ghcr.io/vmosoti/rusticdoc:latest`. Use a version, `ghcr.io/vmosoti/rusticdoc:0.1.0`, once it is published. Until then, load an image you built into your cluster |
| `RUSTICDOC_STORE` | Your bucket and prefix. It says `s3://my-bucket/rusticdoc` |
| `AWS_REGION` | Your bucket's region |
| `AWS_ENDPOINT` | Add it for any store other than AWS, and `AWS_ALLOW_HTTP` if it is plain HTTP |

On EKS with a service account role, drop the two AWS key variables. Read the note on tokens under [security](/docs/operate/security/) first: a compromised LibreOffice runs as the same user and can read a token mounted in the pod.

```bash
kubectl apply -f deploy/kubernetes.yaml
```

## The Deployment

### Pod security

```yaml
automountServiceAccountToken: false
securityContext:
  runAsNonRoot: true
  runAsUser: 1001
  runAsGroup: 1001
  fsGroup: 1001
  seccompProfile: { type: RuntimeDefault }
```

The container's own context adds `allowPrivilegeEscalation: false`, `readOnlyRootFilesystem: true` and drops all capabilities. This matches the [Docker flags](/docs/start/install/). With `automountServiceAccountToken: false`, the pod gets no Kubernetes API token.

### Shutdown

`terminationGracePeriodSeconds: 90` gives a conversion in progress time to finish when a pod is replaced. The service stops accepting connections on `SIGTERM` and finishes what it has. A batch can run for up to `RUSTICDOC_BATCH_TIMEOUT`, 5 minutes by default, so raise the grace period if you run long batches and need them to finish.

### Environment

`RUSTICDOC_STORE`, `AWS_REGION` and `RUSTICDOC_FONT_CACHE_DIR` are plain values. The credentials come from the secret: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `RUSTICDOC_API_KEYS`, `RUSTICDOC_ADMIN_KEYS` and `RUSTICDOC_WEBHOOK_SECRET`.

### Probes

```yaml
readinessProbe:
  httpGet: { path: /ready, port: http }
  periodSeconds: 5
  timeoutSeconds: 5
  failureThreshold: 2
livenessProbe:
  httpGet: { path: /health, port: http }
  initialDelaySeconds: 20
  periodSeconds: 20
  timeoutSeconds: 6
```

Both routes are open without credentials and then show only the status. `/ready` waits for this pod's copy of the stored fonts, so the pod gets no documents before they arrive. It answers `503` for as long as a new pod cannot reach the store. Keep liveness on `/health`, or a store outage would restart pods that have not synced yet, to no benefit.

### Resources

```yaml
resources:
  requests: { cpu: "1", memory: 2Gi }
  limits: { memory: 6Gi }
```

Each pod requests 2 GiB and may grow to 6 GiB. The request is what the scheduler reserves, and the limit fits the service, two LibreOffice workers and four render workers at their limit.

A node needs more than the sum of the requests if pods will really use their limit. Three pods sharing about 6 GB were killed for memory, and their conversions timed out. [Sizing](/docs/operate/sizing/) shows the arithmetic and how to lower the limit with `RUSTICDOC_RENDER_WORKERS` and `RUSTICDOC_RENDER_MEMORY`.

### Volumes

```yaml
volumes:
  - name: tmp
    emptyDir: { sizeLimit: 2Gi }
  - name: home
    emptyDir: { sizeLimit: 512Mi }
  - name: font-copy
    emptyDir: { sizeLimit: 2Gi }
```

All three belong to the pod, and nothing in them has to survive it or be shared. They are mounted at `/tmp`, `/home/rusticdoc` and `/var/cache/rusticdoc`. A new pod rebuilds its font copy from the bucket. The store's default limit on fonts, `RUSTICDOC_MAX_FONT_STORAGE`, is 1 GiB, so the 2 GiB `font-copy` volume holds it. If you raise that limit, raise this one.

## The Service

A plain `ClusterIP` Service on port 9027, with no Ingress. The service speaks HTTP, so TLS and rate limiting are for whatever you put in front of it.

## The PodDisruptionBudget

`minAvailable: 1` keeps at least one pod up through a voluntary disruption such as a node drain.

## What to check after you apply it

- `kubectl get pods` shows three ready pods.
- Store a template through the Service and render it several times, so that more than one pod answers. See [stored templates](/docs/guides/stored-templates/).
- Upload a font with an admin key, wait for `RUSTICDOC_STORE_SYNC_INTERVAL`, and list `/v1/fonts`.
- Watch memory under your own documents, using [`/metrics`](/docs/operate/observability/) and your cluster's tools.
