# Deployment Guide

Pipeline is a single Rust/actix-web service that shells out to `clang`, so the
backend needs a Linux environment with `clang`, `objdump` (binutils), Docker, and
enough RAM for the compiler. This is the full reference; for a quick
step-by-step split deploy see [DEPLOY.md](DEPLOY.md).

## TL;DR

- **Frontend (static):** Cloudflare Pages (recommended) or Netlify.
- **Backend (Linux + clang + Docker):** Vercel (Container Runtime, Hobby) or
  Render (free).
- **RAM:** 512 MB is the practical floor; 1 GB is recommended. On a 512 MB host
  keep `MAX_CONCURRENT_COMPILES=1` (the adaptive default does this for you).

## Deployment shapes

Pipeline supports two shapes.

### 1. Single container

Build one image that contains the backend and the built frontend. The backend
serves the API and the SPA from `FRONTEND_DIR` (the Dockerfile sets it to
`/app/frontend`). This is the simplest option and works on any Docker host -
including the Vercel container, which then serves the frontend too.

No frontend build-time configuration is required here: with `VITE_API_BASE_URL`
unset, the SPA calls the API at the relative path `/api` on its own origin.

### 2. Split deploy

Host the built frontend on a static CDN (Cloudflare Pages or Netlify) and run the
backend container for the API only. Point the frontend at the backend with
`VITE_API_BASE_URL` at build time, and leave `FRONTEND_DIR` unset (or missing) so
the backend serves API only. Step-by-step instructions: [DEPLOY.md](DEPLOY.md).

## Docker (single container)

```bash
# Build (multi-stage: Rust backend + Vite frontend + minimal runtime)
docker build -t pipeline .

# Run with isolation flags (recommended for /api/run - arbitrary code execution)
docker run -p 8080:8080 --memory 1g --cpus 1.0 --pids-limit 256 --network none pipeline

# Health check
curl http://localhost:8080/api/health
```

The image sets `RATE_LIMIT_PER_MINUTE=30` and `MAX_AST_BYTES=2097152`; `WORKERS`
and `MAX_CONCURRENT_COMPILES` adapt to the detected memory limit (see
[512 MB profile](#512-mb-profile-adaptive-low-memory-defaults)). It also defines
a Docker `HEALTHCHECK` that polls `GET /api/health`. Use `--memory 1g` (512 MB is
the floor). Memory is enforced by the container limit, not by `RLIMIT_AS`.

The image runs the server under **tini** (`ENTRYPOINT ["/usr/bin/tini", "--"]`),
so tini is PID 1 and reaps reparented orphans. This matters because the server is
otherwise PID 1: a killed fork bomb would leave zombies that permanently exhaust
the container's PID budget and wedge it. If you override the entrypoint, pass
`--init` to `docker run` instead.

> **`POST /api/run` executes arbitrary user code.** Run the container with
> `--network none --pids-limit 256 --memory 1g` (see [SECURITY.md](SECURITY.md)),
> or disable the endpoint entirely with `ENABLE_EXECUTION=false`. The backend
> additionally applies per-run CPU, memory, process-count and file-size limits
> (`MAX_PROGRAM_MEMORY_MB`, `MAX_PROGRAM_PROCESSES`) and kills the whole process
> group on timeout or output cap. Keep `MAX_PROGRAM_PROCESSES` **below** the
> container's `--pids-limit`.

## 512 MB profile (adaptive low-memory defaults)

The backend inspects the container's memory limit at startup, in priority order:

1. cgroup v2: `/sys/fs/cgroup/memory.max`
2. cgroup v1: `/sys/fs/cgroup/memory/memory.limit_in_bytes`
3. `/proc/meminfo` (`MemTotal:`)

If the detected limit is below **1024 MB**, the server automatically uses the
smallest profile: `WORKERS=1` and `MAX_CONCURRENT_COMPILES=1`. On a larger (or
unknown/unlimited) limit it keeps the defaults `WORKERS=2` /
`MAX_CONCURRENT_COMPILES=2`. Setting `LOW_MEMORY=1` forces the smallest profile
regardless of detection, and explicit `WORKERS` / `MAX_CONCURRENT_COMPILES`
values always win.

Startup logs the decision, for example:

```
Detected memory limit: 512 MB -> WORKERS=1, MAX_CONCURRENT_COMPILES=1
```

**Why this matters:** the backend process itself idles at only ~6-11 MB; the RAM
cost is the `clang` child processes. One worker plus one concurrent compile means
at most **one** `clang` runs at a time. On a 512 MB box, **keep
`MAX_CONCURRENT_COMPILES=1`** (do not raise it).

Child output is also bounded: the AST JSON (`-ast-dump=json`) stdout is read with
a cap of `MAX_AST_BYTES` and stderr with a smaller cap, so a runaway `clang`
cannot make the Rust process buffer unbounded output. When a cap is reached the
child is terminated and the captured output is truncated; the JSON response shape
is unchanged.

```bash
# Force the smallest profile explicitly (useful on 512 MB hosts)
docker run -p 8080:8080 --memory 512m -e LOW_MEMORY=1 pipeline
```

## Deploy to Vercel (Container Runtime)

Vercel can run the whole app as a container Function on Fluid compute (Beta,
available on all plans including Hobby). This path needs the repo-root
**`Dockerfile.vercel`** - Vercel builds that file, not the root `Dockerfile` -
plus the root `vercel.json`, which registers a `container` service and rewrites
every request to it.

The container must listen on **port 80** and bind `0.0.0.0`. `Dockerfile.vercel`
sets `ENV PORT=80` (the app binds `0.0.0.0:$PORT` when `BIND_ADDRESS` is unset),
and `setcap cap_net_bind_service=+ep` lets the non-root `pipeline` user bind port
80. To use a different port, set `PORT` in the Vercel project settings - it
overrides the image default. Vercel respects the image `ENTRYPOINT`, so tini
still runs as PID 1 and reaps orphans.

```bash
# From the repo root
vercel deploy --prod
```

Or connect the repository in the Vercel dashboard; Vercel detects `vercel.json`
automatically. No `buildCommand`/`outputDirectory` is used - the container build
is entirely defined by `Dockerfile.vercel`.

### Caveats (be honest about these)

- **Hobby is non-commercial / personal use only.** Commercial use needs a paid
  plan.
- The Hobby allowance is **4 Active-CPU-hours + 360 GB-hours of provisioned
  memory per month**. Compilation is CPU-bound, so `/api/compile` and `/api/run`
  burn Active CPU fast - expect to exhaust the allowance quickly.
- **300 s** maximum request duration; **5 min** idle scale-to-zero (the next
  request pays a cold start).
- **2 GB RAM / 1 vCPU** on Hobby.
- **4.5 MB** maximum request body (our own JSON limit is 256 KB, so this is not a
  practical constraint here).
- Because compilation is CPU-heavy, consider setting `ENABLE_EXECUTION=false` to
  disable `/api/run` and reduce CPU burn.

## Deploy to Render (Blueprint)

Render's free Docker web service injects a `PORT` env var and expects the app to
listen on it. The backend handles this automatically: when `BIND_ADDRESS` is
unset it binds `0.0.0.0:$PORT` (see
[Environment variables in production](#environment-variables-in-production)). The
repo-root `render.yaml` Blueprint configures the rest.

1. Push the repository to GitHub, then in the Render dashboard choose
   **New > Blueprint** and connect the repository.
2. Render detects `render.yaml` and builds the root `Dockerfile`
   (`runtime: docker`, `plan: free`). No build or start command is needed.
3. Render injects `PORT`; the app binds it. The Blueprint sets `LOW_MEMORY=1`
   (512 MB box -> one worker, one concurrent compile) and `LOG_LEVEL=info`. It
   deliberately does **not** set `PORT` (Render provides it) or `FRONTEND_DIR`
   (the Dockerfile already sets `/app/frontend` for the single-container deploy).
4. Health checks use `GET /api/health` (`healthCheckPath`).
5. The **free instance sleeps after ~15 minutes idle**; the next request pays a
   cold start (a few seconds). It has 512 MB RAM, so keep
   `MAX_CONCURRENT_COMPILES=1` (the adaptive default already does this on a
   512 MB container).

Click **Create**; Render prints an `https://<name>.onrender.com` URL.

To wire it up by hand instead: create a **Web Service** on
[render.com](https://render.com), connect the GitHub repository, choose the
**Docker** environment, and set the health check path to `/api/health`.

## Static frontend (Cloudflare Pages or Netlify)

The split deploy is covered step by step in [DEPLOY.md](DEPLOY.md). In short:

- **Cloudflare Pages** - root directory `pipeline-frontend`, build command
  `npm run build`, output directory `dist`, and a build-time env var
  `VITE_API_BASE_URL=https://<backend-host>/api`.
- **Netlify** - the repo-root `netlify.toml` already sets the base to
  `pipeline-frontend`, the build command to `npm run build`, and the publish
  directory to `dist`; add the same `VITE_API_BASE_URL` env var.

`VITE_API_BASE_URL` is **build-time** (Vite bakes it in), so changing it requires
a rebuild and redeploy.

### Why not Cloudflare Workers for the backend

Workers run V8 isolates, not native binaries: they cannot run `clang` or Docker,
have no writable filesystem for temp objects, and have a small memory limit. Use
Cloudflare **Pages** for the frontend only, and a container host for the backend.

## Keeping a free instance warm

A scale-to-zero instance pays a cold start on the first request after idle. An
uptime pinger can avoid that - but only where the free plan is billed by
instance-hours:

- **Render (free): 750 instance-hours/month**, enough for 24/7 on a single
  service. A pinger keeps it warm for free.
- **Vercel (Hobby): do NOT keep it warm.** Vercel bills *provisioned memory*
  while an instance is warm, not just Active CPU. At 2 GB x 24/7 that is roughly
  **1,440 GB-hours/month** against a **360 GB-hour** allowance - the free budget
  would be gone in about a week. On Vercel, accept cold starts (and consider
  `ENABLE_EXECUTION=false` to reduce CPU burn).

## Environment variables in production

Backend (already defaulted in the Docker image; override as needed):

| Variable | Default | Purpose |
| --- | --- | --- |
| `BIND_ADDRESS` | `0.0.0.0:8080` | Address and port to bind. When set, it takes precedence over `PORT`. |
| `PORT` | (unset) | Platform-injected listen port (e.g. Render, Vercel). Used only when `BIND_ADDRESS` is unset; the app then binds `0.0.0.0:$PORT`. |
| `LOG_LEVEL` | `info` | Log filter. |
| `WORKERS` | adaptive: `2`, or `1` when the detected limit is `<1024 MB` | actix worker threads. Keep low on small instances. |
| `MAX_CONCURRENT_COMPILES` | adaptive: `2`, or `1` when the detected limit is `<1024 MB` | Global compile/run/execute concurrency cap. **Must stay `1` on a 512 MB box.** |
| `LOW_MEMORY` | (unset) | Set to `1` to force the smallest profile (`WORKERS=1`, `MAX_CONCURRENT_COMPILES=1`) regardless of detection. |
| `FRONTEND_DIR` | `./frontend` | Set to the built SPA directory for single-container deploys; leave unset for split deploys. |
| `RATE_LIMIT_PER_MINUTE` | `30` | Per-IP limit for `/api/*`; `0` disables. |
| `MAX_AST_BYTES` | `2097152` | Cap on captured clang AST JSON (also the stdout read cap for the AST stage). |
| `ENABLE_EXECUTION` | `true` | Kill switch for `POST /api/run` (arbitrary code execution). Set to `false` to disable it (returns 403). |
| `MAX_PROGRAM_MEMORY_MB` | `256` | Address-space limit (`ulimit -v`) for executed programs. Clamped 16-4096. |
| `MAX_PROGRAM_PROCESSES` | `128` | Process-count limit (`ulimit -u`) for executed programs. Clamped 1-4096. Keep below the container `--pids-limit`. |
| `TRUST_PROXY` | `true` | Resolve the client IP from `X-Forwarded-For` (first entry), then `X-Real-IP`, for the rate limiter and run cooldown. Set to `false` if the app is exposed **directly** (the headers are spoofable). |
| `MAX_SOURCE_BYTES` | `16384` | Reject `source_code` larger than this on `/api/compile` and `/api/run`. |
| `RUN_COOLDOWN_SECS` | `180` | Per-IP cooldown after an accepted `POST /api/run`; further runs return `429` with `Retry-After`. `0` disables. |

Frontend (build-time): leave `VITE_API_BASE_URL` unset for a single-container
deploy (the app defaults to the same-origin relative path `/api`). For a split
deploy, set it to an absolute URL pointing at the backend, for example
`https://<backend-host>/api`. Rebuild the frontend after changing it.

## Verifying a deployment

```bash
# Platform/container health
curl https://<backend-host>/api/health
# {"status":"healthy","timestamp":"...","version":"0.1.0"}

# End-to-end compile
curl -s https://<backend-host>/api/compile \
  -H 'Content-Type: application/json' \
  -d '{"source_code":"int main(){return 0;}"}' | head -c 200

# End-to-end run (unless ENABLE_EXECUTION=false)
curl -s https://<backend-host>/api/run \
  -H 'Content-Type: application/json' \
  -d '{"source_code":"int main(){return 0;}"}'
```

## Notes

- **CORS:** the backend allows any origin and exposes response headers (so a
  split deploy can read `Retry-After` on a `429`). Tighten this if you expose
  the API publicly.
- **Cold starts:** scale-to-zero providers (Vercel Hobby, Render free) start a
  container on the first request after idle; expect a short delay.
- **HTTPS:** all recommended platforms provide managed TLS.
- **Windows:** the backend compiles and runs, but C compilation is Unix-only
  (`cfg(unix)`), so deploy on Linux.
