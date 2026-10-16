# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 1.0.x   | :white_check_mark: |

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

If you discover a security vulnerability, please email the maintainers directly or open a private security advisory on GitHub.

Include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

We will respond within 48 hours and work with you to address the issue promptly.

## Security Measures

Pipeline implements several security measures:

1. **Sandboxed Execution**: Code runs in isolated Docker containers
2. **Resource Limits**: per-request timeout (compile) and per-run CPU, memory,
   process-count and file-size limits for executed programs
3. **Input Validation**: source code size and line limits, JSON body limits
4. **No Unsafe Code**: no `unsafe` blocks in the Rust backend
5. **Rate Limiting**: per-IP limiter on `/api/*`
6. **CORS Protection**: configurable CORS policies

## Arbitrary code execution (`POST /api/run`)

`POST /api/run` compiles **and links** the submitted C source into a native
executable and runs it, returning the program's real stdout/stderr/exit code.
This is an **arbitrary code execution** surface: treat it as hostile.

Mitigations implemented by the backend:

- Wall-clock timeout per run (default 5 s, clamped 1–10 s); the child is killed
  when it expires.
- CPU-time limit (`ulimit -t`).
- Address-space limit (`ulimit -v`, `MAX_PROGRAM_MEMORY_MB`, default 256 MB).
- Process-count limit (`ulimit -u`, `MAX_PROGRAM_PROCESSES`, default 128) — this
  contains fork bombs. Keep it **below** the container's `--pids-limit`.
- File-size limit (`ulimit -f 8192`, i.e. 4 MiB).
- Output caps: 64 KiB per stream; the whole process group is killed as soon as
  either cap is reached.
- Each run executes in its own process group; on timeout or output cap the
  **group** is killed so forked grandchildren cannot survive.
- Runs share the global compile semaphore and the per-IP rate limiter, and use a
  per-request temp dir that is removed on every path (including client
  disconnects).
- `ENABLE_EXECUTION=false` disables the endpoint entirely (returns 403).

### The container MUST run with an init

The server is **PID 1** inside the container. When a run's process group is
killed, any surviving children (e.g. from a fork bomb) are reparented to PID 1;
if nothing reaps them they become **zombies** and permanently consume the
container's PID budget — `pids.current` never drops and the container wedges
(even `docker exec` fails). **Always run an init as PID 1:**

- The image ships with **tini** (`ENTRYPOINT ["/usr/bin/tini", "--"]`), so the
  default `docker run` is already safe.
- If you override the entrypoint, add `--init` to `docker run` so Docker inserts
  its own init as PID 1.

### Required deployment flags

The backend limits are defence in depth, not a sandbox. Run the container with
isolation flags:

```bash
docker run --network none --pids-limit 256 --memory 1g --cpus 1.0 \
  -p 8080:8080 pipeline
```

- `--network none` — user programs get no network access.
- `--pids-limit 256` — caps the total number of processes in the container.
  Keep `MAX_PROGRAM_PROCESSES` (default 128) **below** this value.
- `--memory 1g` — caps container memory.
- `--cpus 1.0` — caps CPU.
- `--init` — add this **only if you override the image entrypoint**, so Docker
  provides PID 1 and reaps orphans (the image already runs tini).
- Set `ENABLE_EXECUTION=false` to turn `/api/run` off completely.

The container already runs as a non-root user.

## Best Practices for Deployment

- Always run backend in Docker container
- Set appropriate resource limits
- Use HTTPS in production
- Implement rate limiting
- Monitor for unusual activity
- Keep dependencies updated

## Responsible Disclosure

We follow responsible disclosure practices and will:
- Acknowledge your report within 48 hours
- Provide a fix timeline
- Credit you in release notes (if desired)
- Keep you informed throughout the process
