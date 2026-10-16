# DEPLOY.md - split deploy (static frontend + Vercel backend)

A practical, step-by-step path: host the built frontend on a CDN (Cloudflare
Pages, or Netlify) and run the backend container on Vercel. The frontend calls
the backend over HTTPS.

If you would rather not split, skip to
[Single-container option](#single-container-option) - the Vercel container can
serve the built frontend too.

See [DEPLOYMENT.md](DEPLOYMENT.md) for other hosts and the full env-var tables.

## Prerequisites

- **Push the code to GitHub.** This working copy has local commits but **no git
  remote yet**, so create an empty repository on GitHub and push:

  ```bash
  git remote add origin https://github.com/<you>/<repo>.git
  git push -u origin main
  ```

  Vercel and Cloudflare Pages / Netlify all deploy from a connected Git repo.

## 1. Backend -> Vercel (Container Runtime, Hobby)

1. Sign up / log in at https://vercel.com with GitHub.
2. **Add New... -> Project**, then import your repository.
3. Vercel detects the repo-root `vercel.json` and `Dockerfile.vercel` and builds
   the backend as a **container Function**. Accept the defaults - no build
   command or output directory is needed.
4. The container listens on **port 80** by default. To use a different port, set
   a `PORT` environment variable in the project settings (Settings ->
   Environment Variables).
5. **Deploy** and copy the resulting URL: `https://<project>.vercel.app`.

### Optional environment variables

Set these in the Vercel project settings (most apply on the next deploy or
restart):

| Variable | Default | Purpose |
| --- | --- | --- |
| `LOG_LEVEL` | `info` | Use `debug` for more logs while testing. |
| `ENABLE_EXECUTION` | `true` | Set `false` to disable `POST /api/run` (no arbitrary code execution) and cut CPU burn. |
| `RUN_COOLDOWN_SECS` | `180` | Per-IP cooldown after a run; `0` disables it. |
| `MAX_SOURCE_BYTES` | `16384` | Reject larger source on compile/run. |
| `TRUST_PROXY` | `true` | **Leave `true` on Vercel** - it resolves the real client IP from `X-Forwarded-For`. |

### Caveats (read these)

- The Container Runtime is **Beta**.
- The first request after **5 minutes idle** pays a **cold start** (a few
  seconds; occasionally a 502 - just retry).
- The Hobby allowance is **4 Active-CPU-hours + 360 GB-hours of provisioned
  memory per month**. Compilation is CPU-bound, so heavy traffic burns it fast.
- **Do not add an uptime pinger.** Vercel bills provisioned memory while an
  instance is warm: 2 GB x 24/7 is about 1,440 GB-hours/month, which would
  exhaust the 360 GB-hour budget in roughly a week. Accept cold starts instead.
- Hobby is for **personal / non-commercial use**.

## 2. Frontend -> Cloudflare Pages (recommended)

1. In the Cloudflare dashboard: **Workers & Pages -> Create -> Pages ->
   Connect to Git**, then pick the same repository.
2. Build settings:
   - **Root directory:** `pipeline-frontend`
   - **Build command:** `npm run build`
   - **Output directory:** `dist`
3. Add a **build-time** environment variable:
   - `VITE_API_BASE_URL = https://<project>.vercel.app/api`
4. Save and deploy. Cloudflare Pages is free, has unlimited bandwidth, and needs
   no card. (A `pipeline-frontend/wrangler.toml` is committed too, if you prefer
   deploying from the CLI with `npx wrangler pages deploy`.)

### Alternative: Netlify

1. **Add new site -> Import an existing project**, then pick the repository.
2. Netlify reads the repo-root `netlify.toml`, which already sets the base
   directory to `pipeline-frontend`, the build command to `npm run build`, and
   the publish directory to `dist` (you can also set these in the Netlify UI).
3. Add the same build-time env var:
   - `VITE_API_BASE_URL = https://<project>.vercel.app/api`

### `VITE_API_BASE_URL` is build-time

Vite bakes the value into the bundle when the frontend is built, so changing it
requires a **rebuild and redeploy** - it is not read at runtime.

## Single-container option

If you would rather not split, the Vercel container already serves the built
frontend (the image sets `FRONTEND_DIR=/app/frontend`), so you can use the
`https://<project>.vercel.app` URL on its own - no separate frontend host needed.

## Verify

1. Open the frontend URL (the Cloudflare Pages / Netlify one, or the Vercel URL
   for the single-container option).
2. Click **Compile** and confirm the compilation stages render.
3. Open the **Output** tab and run the program; the real stdout/stderr should
   appear (unless you set `ENABLE_EXECUTION=false`).
4. If something fails, check the **Vercel function logs** (Project -> Logs) and
   the browser console.

## Troubleshooting

| Symptom | Likely cause / fix |
| --- | --- |
| 502 on the first load | Cold start. Wait a few seconds and retry. |
| API calls fail cross-origin | `VITE_API_BASE_URL` must end in `/api` and must have been set **before** the frontend build. Rebuild after changing it. |
| 503 on Compile | The container has no compiler or is still warming up. Retry and check the logs. |
| 429 on Run | Per-IP run cooldown (`RUN_COOLDOWN_SECS`). Wait for `Retry-After`, or set it to `0`. |
| 403 on Run | `ENABLE_EXECUTION=false` is set. |

For the `/api/run` execution surface and its limits, see
[SECURITY.md](SECURITY.md).
