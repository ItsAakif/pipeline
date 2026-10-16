# Pipeline - C Compiler Visualization Tool

An interactive educational platform that visualizes the whole C compilation
pipeline - from source code through every compiler stage - and then compiles,
links and runs the program so you can see its real output. Built for students,
educators, and anyone curious about how compilers work.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Docker](https://img.shields.io/badge/Docker-Ready-2496ED?logo=docker)](Dockerfile)

## Overview

You write C in a browser editor; the backend shells out to `clang` (and
`objdump`) and returns each compilation stage as structured data. The frontend
renders the stages, and the **Output** tab compiles, links and runs your program
inside the container to show its real stdout, stderr and exit code.

Pipeline is a single Rust/actix-web service that can either serve the built
frontend itself (single container) or run as an API-only backend behind a static
frontend (split deploy). See [DEPLOY.md](DEPLOY.md) for a step-by-step split
deploy and [DEPLOYMENT.md](DEPLOYMENT.md) for the full reference.

## Architecture

```
 browser (SolidJS SPA)
   |  POST /api/compile -> every compilation stage
   |  POST /api/run     -> compile + link + run; real stdout/stderr/exit code
   v
 actix-web backend (Rust)
   |  writes source.c into a per-request temp dir, then runs child processes
   |  with timeouts, output caps and resource limits
   v
 clang  /  objdump  /  the compiled program
```

Two deploy shapes: **single container** (the backend also serves the built SPA)
or **split** (static frontend on a CDN, backend container for the API only).

## Features

### Compilation stages

- **Tokenization** - tokens with type, position, and beginner-friendly notes
- **Abstract Syntax Tree** - tree view with per-stage download (image/JSON)
- **Symbol table** - variables, functions, parameters, and their scopes
- **LLVM IR** - unoptimized (`-O0`) and optimized (`-O2`)
- **Assembly** - Intel-syntax x86-64 with per-instruction annotation
- **Binary** - the compiled object file as hex
- **Decoded instructions** - the real `.text` section, broken down into
  prefix / opcode / ModR/M / SIB
- **O0 vs O2 diff** - side-by-side comparison of optimized and unoptimized output

### Program output

- The **Output** tab compiles, **links** and runs your program in the container
  and shows its real **stdout**, **stderr** and **exit code** (`POST /api/run`)
- A non-zero exit code is still a successful run - the code is reported as-is
- Sandboxed and bounded: wall-clock timeout, CPU / memory / process-count /
  file-size limits, per-stream output caps, a non-root user, and a per-IP
  cooldown. It can be turned off with `ENABLE_EXECUTION=false`.

### Editor and workflow

- Monaco editor with C syntax highlighting
- 9 built-in examples (Hello World, Simple Math, For Loop, Function Call,
  Array Operations, Pointers, Recursion, Structs, Bit Operations)
- Save/load code as JSON; auto-save to browser storage
- Permalink sharing (code and active stage encoded in the URL)
- Dark / light theme and keyboard shortcuts (`Ctrl`/`Cmd`+`Enter` to compile,
  `Ctrl`/`Cmd`+`S` to copy a share link, `?` for help)

## Quick Start

### Docker (single container, recommended)

The image builds the backend and the frontend, and the backend serves both on
port 8080.

```bash
git clone https://github.com/yourusername/pipeline.git
cd pipeline

docker build -t pipeline .
docker run -p 8080:8080 --memory 1g pipeline

# Open http://localhost:8080
```

The container defines a health check at `GET /api/health`.

### Local development

Prerequisites: a Rust toolchain (2021 edition), Node.js 20+, and `clang` plus
`objdump` (binutils) on `PATH`. Compilation support is Unix-only; Windows is not
supported at runtime by design.

Backend (listens on `0.0.0.0:8080` by default):

```bash
cd pipeline-backend
cargo run
```

Frontend (Vite dev server on port 3000, proxying `/api` and `/ws` to the
backend):

```bash
cd pipeline-frontend
npm install
npm run dev
```

Then open http://localhost:3000. Copy `pipeline-frontend/.env.example` to `.env`
to override the API base URL.

## API

All endpoints live under `/api`; a WebSocket endpoint is at `/ws`. Request
bodies are JSON and are capped at 256 KB (oversized bodies are rejected with
`413`).

### POST /api/compile

Compile C source and return every compilation stage.

```json
{
  "source_code": "int main() { return 0; }",
  "compiler": "clang",
  "flags": [],
  "timeout_secs": 10
}
```

Fields: `source_code` (required), `compiler` (`clang` or `gcc`, default
`clang`), `flags` (array, default `[]`), `timeout_secs` (1-60, default `10`).
Source code is limited to **100 lines** and **16 KB** (`MAX_SOURCE_BYTES`).

Response:

```json
{
  "success": true,
  "assembly": "...",
  "source": "...",
  "binary": "<base64 object file>",
  "stages": {
    "tokens": [],
    "ast": null,
    "symbols": { "scopes": [] },
    "ir_unoptimized": "",
    "ir_optimized": "",
    "optimizations_applied": [],
    "assembly": "",
    "assembly_annotated": [],
    "binary": [],
    "binary_decoded": []
  },
  "error": null,
  "compilation_time_ms": 234
}
```

### POST /api/run

Compile, **link** and run the program in the container, and return its real
output. This is what the UI's **Output** tab uses.

```json
{ "source_code": "int main() { return 0; }", "timeout_secs": 5 }
```

Fields: `source_code` (required; the same 100-line / 16 KB limits as
`/api/compile`) and `timeout_secs` (default `5`, clamped to 1-10).

Response:

```json
{
  "success": true,
  "compile_error": null,
  "stdout": "Hello, World!\n",
  "stderr": "",
  "exit_code": 0,
  "timed_out": false,
  "truncated": false,
  "execution_time_ms": 12
}
```

- `success` is `false` only when the program could not be built; `compile_error`
  then carries the compiler diagnostics.
- A program that exits non-zero is still `success: true` - read `exit_code`.
- `timed_out` is `true` when the run hit the wall-clock timeout; `truncated` is
  `true` when output was cut short (a stream cap, or a timeout).
- `403` when execution is disabled (`ENABLE_EXECUTION=false`).
- `429` (with `Retry-After`) while a per-IP run cooldown (`RUN_COOLDOWN_SECS`)
  is active.

### POST /api/execute (legacy)

**Not used by the UI.** This is the original instruction-simulation endpoint: it
decodes a base64 object file (as returned by `/api/compile`) and returns a
simulated execution timeline of frames. It is kept for API compatibility and is
still tested.

```json
{ "binary": "<base64 object file>" }
```

Response:

```json
{
  "success": true,
  "frames": [],
  "frame_count": 150,
  "error": null,
  "execution_time_ms": 567
}
```

### GET /api/health

Returns `{ "status": "healthy", "timestamp": "<RFC 3339>", "version": "<crate version>" }`.
Used by container and platform health checks.

### GET /ws

WebSocket endpoint for client keepalive and subscriptions (`ping`/`pong` and
`subscribe_execution`). The current UI does not use it; it is available for
streaming-style clients.

### Errors

- `400` - invalid request (empty source, over 100 lines, over 16 KB, unsupported
  compiler; on `/api/compile`, `timeout_secs` outside 1-60)
- `403` - `POST /api/run` when execution is disabled (`ENABLE_EXECUTION=false`)
- `413` - request body larger than 256 KB
- `429` - per-IP rate limit, or an active `/api/run` cooldown; the response
  includes `Retry-After` and a JSON body with `retry_after_secs`
- `503` - no C compiler available on the server, or the service is shutting down

### Static frontend

When `FRONTEND_DIR` points at a built frontend, every non-`/api`, non-`/ws` GET
is served from that directory with an SPA fallback to `index.html`. If the
directory is missing, the backend serves the API only.

## Environment variables

### Backend

| Variable | Default | Purpose |
| --- | --- | --- |
| `BIND_ADDRESS` | `0.0.0.0:8080` | Address and port to bind. Takes precedence over `PORT` when set. |
| `PORT` | (unset) | Platform-injected listen port (e.g. Render, Vercel). Used when `BIND_ADDRESS` is unset; the app binds `0.0.0.0:$PORT`. |
| `LOG_LEVEL` | `info` | Log filter: `trace`, `debug`, `info`, `warn`, `error`. |
| `WORKERS` | adaptive: `2`, or `1` on a `<1024 MB` host | Number of actix worker threads (minimum 1). |
| `MAX_CONCURRENT_COMPILES` | adaptive: `2`, or `1` on a `<1024 MB` host | Global concurrency cap for compile/run/execute (minimum 1). |
| `LOW_MEMORY` | (unset) | Set to `1` to force the smallest profile (`WORKERS=1`, `MAX_CONCURRENT_COMPILES=1`). |
| `FRONTEND_DIR` | `./frontend` | Directory of the built SPA to serve; omit for API-only. |
| `RATE_LIMIT_PER_MINUTE` | `30` | Per-IP request budget for `/api/*`; `0` disables. |
| `MAX_AST_BYTES` | `2097152` | Cap on captured clang AST JSON (2 MiB). |
| `ENABLE_EXECUTION` | `true` | Kill switch for `POST /api/run` (arbitrary code execution); `false` disables it (403). |
| `MAX_PROGRAM_MEMORY_MB` | `256` | Address-space limit (`ulimit -v`) for executed programs (clamped 16-4096). |
| `MAX_PROGRAM_PROCESSES` | `128` | Process-count limit (`ulimit -u`) for executed programs (clamped 1-4096). Keep below the container `--pids-limit`. |
| `TRUST_PROXY` | `true` | Take the client IP from `X-Forwarded-For` (first entry), then `X-Real-IP`, for the rate limiter and run cooldown. Set to `false` when the app is exposed **directly** - the headers are spoofable. |
| `MAX_SOURCE_BYTES` | `16384` | Reject `source_code` larger than this on `/api/compile` and `/api/run` (16 KiB). |
| `RUN_COOLDOWN_SECS` | `180` | Per-IP cooldown after an accepted `/api/run`; further runs get `429` + `Retry-After`. `0` disables. |

`POST /api/run` compiles, links and **executes** user code, so it is an
arbitrary-execution surface. Run the container with
`--network none --pids-limit 256 --memory 1g`, or set `ENABLE_EXECUTION=false`.
The container **must run with an init** - the image uses tini as PID 1 (or pass
`--init` if you override the entrypoint) - otherwise a fork bomb leaves zombies
that wedge the container. Keep `MAX_PROGRAM_PROCESSES` (default 128) below the
container's `--pids-limit`. See [SECURITY.md](SECURITY.md) for the full list of
mitigations.

At startup the backend detects the container memory limit (cgroup v2 -> cgroup v1
-> `/proc/meminfo`) and, below 1024 MB, automatically uses `WORKERS=1` and
`MAX_CONCURRENT_COMPILES=1` so at most one `clang` runs at a time. `LOW_MEMORY=1`
forces the smallest profile; explicit `WORKERS` / `MAX_CONCURRENT_COMPILES`
values always override. See [DEPLOYMENT.md](DEPLOYMENT.md#512-mb-profile-adaptive-low-memory-defaults)
for the 512 MB profile.

### Frontend

| Variable | Default | Purpose |
| --- | --- | --- |
| `VITE_API_BASE_URL` | `/api` (same-origin) | Backend API base URL. Leave unset when the frontend is served from the same origin as the backend (single-container deploy) - the app then uses the relative path `/api`. Set an absolute URL such as `https://<backend-host>/api` for a split deploy. Read at build time; rebuild after changing it. |

`pipeline-frontend/.env.example` also documents a commented `VITE_WS_BASE_URL` for
future use; the current UI does not consume it yet.

## Tech stack

**Backend**

- Rust (2021 edition), actix-web 4, tokio
- `clang`/LLVM and `objdump` (binutils) for compilation, linking and disassembly
- iced-x86 for instruction decoding; `object` for ELF section parsing
- lru compilation cache, sha2 hashing, serde JSON

**Frontend**

- SolidJS 1.8 + TypeScript 5
- Vite 5
- Monaco Editor
- html2canvas (AST image export)

## Testing

Backend:

```bash
cd pipeline-backend
cargo test
cargo clippy
```

Frontend (type checking and production build):

```bash
cd pipeline-frontend
npm run lint        # tsc --noEmit
npm run type-check  # tsc --noEmit (alias)
npm run build
```

## Deployment

Pipeline supports two deploy shapes:

1. **Single container** - build the image and run it; the backend serves the API
   and the built frontend from `FRONTEND_DIR`.
2. **Split** - host the static frontend on a CDN (Cloudflare Pages or Netlify)
   and run the backend container on Vercel or Render.

- [DEPLOY.md](DEPLOY.md) - step-by-step split deploy (Vercel + Cloudflare Pages).
- [DEPLOYMENT.md](DEPLOYMENT.md) - full reference: single-container, Vercel,
  Render, the 512 MB adaptive profile, the environment variables, and the
  security / deployment-flag notes.

## Known limitations

- Compilation is Unix-only (`cfg(unix)`); Windows is not supported at runtime.
- Targets x86-64 Linux ELF binaries.
- Maximum 100 lines and 16 KB of C source per request.
- The Output tab really compiles, links and runs the program inside the
  container, sandboxed (wall-clock timeout, CPU / memory / process / file-size
  limits, output caps, non-root user; run the container with `--network none`
  for stronger isolation). The legacy `/api/execute` endpoint, by contrast, is a
  simplified instruction simulation, not a full CPU emulator.
- Requires `clang` and `objdump`; `/api/compile` and `/api/run` return `503` when
  no compiler is available.

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines,
and [SECURITY.md](SECURITY.md) to report vulnerabilities.

## License

MIT License - see [LICENSE](LICENSE).
