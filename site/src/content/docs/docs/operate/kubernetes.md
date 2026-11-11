---
title: Kubernetes
description: What the manifest in deploy/kubernetes.yaml sets up, and what to change before you apply it to your cluster.
sidebar:
  order: 3
---

`deploy/kubernetes.yaml` is a starting point for you to adapt to your cluster.

## Before you apply it

The manifest runs three replicas on one S3-compatible bucket, which is the only thing they share. [Many containers](/docs/operate/many-containers/) explains how that behaves.

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
| `image` | A version, `ghcr.io/crafted-systems/rusticdoc:0.1.3`. The manifest says `latest`, which moves to each new release when a pod restarts |
| `RUSTICDOC_STORE` | Your bucket and prefix, in place of `s3://my-bucket/rusticdoc` |
| `AWS_REGION` | Your bucket's region |
| `AWS_ENDPOINT` | Add it for any store other than AWS, along with `AWS_ALLOW_HTTP` if the endpoint is plain HTTP |

On EKS with a service account role you can drop the two AWS key variables, but read the note on tokens under [security](/docs/operate/security/) first, because a compromised LibreOffice runs as the same user and can read a token mounted in the pod.

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

The container's own context adds `allowPrivilegeEscalation: false` and `readOnlyRootFilesystem: true` and drops all capabilities, which matches the [Docker flags](/docs/start/install/). `automountServiceAccountToken: false` means the pod gets no Kubernetes API token.

### Shutdown

`terminationGracePeriodSeconds: 90` gives a conversion in progress time to finish when a pod is replaced, because on `SIGTERM` the service stops accepting connections and finishes what it has. A batch can run for up to `RUSTICDOC_BATCH_TIMEOUT`, 5 minutes by default, so raise the grace period if you run long batches and need them to finish.

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

Both routes work without credentials, and then show only the status. `/ready` waits for this pod's copy of the stored fonts, so the pod isn't sent any documents before it has them, and it returns `503` for as long as a new pod can't reach the store. Keep liveness on `/health`, or a store outage would restart pods that haven't synced yet, for no benefit.

### Resources

```yaml
resources:
  requests: { cpu: "1", memory: 2Gi }
  limits: { memory: 6Gi }
```

Each pod requests 2 GiB, which is what the scheduler reserves, and can grow to 6 GiB. That limit fits the service, two LibreOffice workers and four render workers at their limit.

If pods are really going to use their limit, a node needs more than the sum of the requests. Three pods sharing about 6 GB were killed for memory, and their conversions timed out. [Sizing](/docs/operate/sizing/) shows the arithmetic, and how to lower the limit with `RUSTICDOC_RENDER_WORKERS` and `RUSTICDOC_RENDER_MEMORY`.

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

All three belong to the pod and are mounted at `/tmp`, `/home/rusticdoc` and `/var/cache/rusticdoc`. Nothing in them has to survive the pod or be shared, and a new pod rebuilds its font copy from the bucket. The store's default limit on fonts, `RUSTICDOC_MAX_FONT_STORAGE`, is 1 GiB, so the 2 GiB `font-copy` volume holds it. If you raise that limit, raise the volume's too.

## The Service

This is a plain `ClusterIP` Service on port 9027, with no Ingress. The service speaks HTTP, so TLS and rate limiting are up to whatever you put in front of it.

## The PodDisruptionBudget

`minAvailable: 1` keeps at least one pod up through a voluntary disruption like a node drain.

## What to check after you apply it

- `kubectl get pods` should show three ready pods.
- Store a template through the Service and render it several times, so that more than one pod handles it. See [stored templates](/docs/guides/stored-templates/).
- Upload a font with an admin key, wait for `RUSTICDOC_STORE_SYNC_INTERVAL`, and list `/v1/fonts`.
- Watch memory while it handles your own documents, using [`/metrics`](/docs/operate/observability/) and your cluster's tools.
