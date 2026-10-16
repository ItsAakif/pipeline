# Contributing to Pipeline

Thank you for your interest in contributing to Pipeline! This document provides guidelines for contributing to the project.

## Code of Conduct

- Be respectful and inclusive
- Welcome newcomers and help them get started
- Provide constructive feedback
- Focus on what is best for the community

## How to Contribute

### Reporting Bugs

If you find a bug, please open an issue with:
- Clear description of the problem
- Steps to reproduce
- Expected vs actual behavior
- Your environment (OS, browser, etc.)

### Suggesting Features

Feature suggestions are welcome! Please:
- Check if it's already been suggested
- Explain the use case
- Describe the expected behavior

### Pull Requests

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Make your changes
4. Test thoroughly
5. Commit with clear messages (`git commit -m 'Add amazing feature'`)
6. Push to your fork (`git push origin feature/amazing-feature`)
7. Open a Pull Request

### Code Standards

**Rust Backend:**
- Follow `rustfmt` formatting
- Run `cargo clippy` and fix warnings
- Add tests for new features
- Document public APIs

**TypeScript Frontend:**
- Use TypeScript strictly
- Follow existing code style
- Keep components modular and focused
- Add prop types and comments

### Testing

**Backend:**
```bash
cd pipeline-backend
cargo test
cargo clippy
```

**Frontend:**
```bash
cd pipeline-frontend
npm run build
npm run lint        # tsc --noEmit
npm run type-check  # tsc --noEmit (alias)
```

## Project Structure

```
Pipeline/
|-- pipeline-backend/     # Rust/actix-web server
|   |-- src/
|   |   |-- api/          # HTTP endpoints (/compile, /run, /execute), rate limiting, static SPA
|   |   |-- compiler/     # clang/objdump orchestration and stage extraction
|   |   |-- execution/    # Legacy instruction simulation (POST /api/execute)
|   |   |-- models/       # Data structures
|   |   |-- memory.rs     # Container memory-limit detection
|   |   `-- main.rs       # Server setup, routing, middleware
|   `-- Cargo.toml
|-- pipeline-frontend/    # SolidJS frontend
|   |-- src/
|   |   |-- components/   # UI components (viewers, editor, OutputView)
|   |   |-- lib/          # API client, sharing, download, Monaco, opcode helpers
|   |   |-- App.tsx       # App shell and stage/output navigation
|   |   `-- types.ts      # Shared types
|   `-- package.json
|-- Dockerfile            # Single-container image (backend + frontend + runtime)
|-- Dockerfile.vercel     # Vercel Container Runtime image (listens on port 80)
|-- vercel.json           # Vercel container service + rewrites
|-- render.yaml           # Render Blueprint
|-- DEPLOY.md             # Step-by-step split deploy (Vercel + Cloudflare Pages)
|-- DEPLOYMENT.md         # Full deployment reference
|-- README.md             # Project overview and quick start
`-- SECURITY.md           # Security policy and /api/run hardening
```

## Development Setup

See README.md for full setup instructions, and DEPLOY.md / DEPLOYMENT.md for
deployment.

## Questions?

Feel free to open an issue for any questions!
