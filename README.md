# Ladoo

A Rust backend framework that's simple to start and safe to ship.

Ladoo prioritizes developer experience without sacrificing performance. Write your first handler in three lines, then graduate to authentication, job queues, and database migrations — same framework, no rewrites.

## Background

This project is the outcome of work I did for another project while working at a company. There were a few bigger problems we started by solving — how our jobs ran and how we ran database migrations. One thing led to another, and it all slowly came together into something I thought would be useful as a framework. So I started working on getting all the pieces together.

Ladoo is a learning project with production ambitions. It's not a toy — every feature here solved a real problem first.

## Quick Start

```rust
use ladoo::prelude::*;

fn main() {
    App::new()
        .get("/", |_: Request| "Hello World")
        .get("/users/:id", |req: Request| {
            let id = req.param("id").unwrap_or("0");
            format!("User {id}")
        })
        .run("0.0.0.0:3000");
}
```

## Features

### Core

- **Routing** — static, parameterized (`:id`), and wildcard (`*path`) routes with method-aware 405 responses
- **Extractors** — `Json<T>`, `Query<T>`, `Path<T>`, `State<T>`, `Auth<T>`, `Valid<T>` — pull typed data from requests
- **Error handling** — one unified `Error` type with dev/prod rendering, `#[derive(AppError)]` for custom errors
- **State & DI** — `.provide(T)` + `State<T>` extractor, `Arc<T>` storage under the hood (no `Clone` bound)
- **Middleware** — plain async functions, composable via groups and route-scoped mounting
- **Configuration** — `#[derive(Config)]` macro with TOML loading and environment detection

### Security & Auth

- **Authentication** — `Auth<T>` extractor with pluggable providers (`ApiKeyAuth`, `JwtAuth`)
- **Authorization** — RBAC with `RequireRole`, `RequirePermission`, and `ResourcePolicy` guards
- **Security headers** — `SecureHeaders` middleware (HSTS, CSP, X-Frame-Options, etc.)
- **CORS** — builder API with preflight handling
- **Rate limiting** — `RateLimit` middleware with `RateKey` and tiered limits, `MemoryStore` backend
- **Hardened defaults** — 2 MiB body limit, JWT expiry enforcement, error sanitization in production

### Data & Storage

- **Validation** — `Valid<T>` composable extractor with `Validate` trait and `validator` crate integration
- **Pagination** — offset and cursor-based with `Paginate` and `CursorParams` extractors
- **Caching** — `CacheStore` trait with `MemoryStore`, `Cache<T>` wrapper with TTL
- **Database migrations** — standalone `ladoo-migrate` crate with CLI, multi-database support (SQLite, Postgres, MySQL), versioned and repeatable migrations, rollback, repair, and baseline

### Infrastructure

- **Job queue** — `Job` trait with `#[derive(Job)]`, `JobRunner` with configurable retries, backoff, and timeouts
- **Health checks** — `HealthCheckable` trait with auto-discovery, configurable endpoint, and error redaction
- **HTTP/2** — automatic h2c (cleartext) via auto-builder swap, feature-gated TLS via rustls with ALPN negotiation
- **Graceful shutdown** — signal handling with connection draining and configurable timeout
- **Plugin system** — `Plugin` trait with shutdown hooks and duplicate detection

### Developer Experience

- **Testing** — `TestClient` for in-memory testing, `TestServer` for real TCP integration tests
- **Prelude** — `use ladoo::prelude::*` brings everything into scope
- **Structured logging** — tracing integration with request IDs and configurable formatting
- **Derive macros** — `#[derive(Config)]`, `#[derive(AppError)]`, `#[derive(Job)]`

## Architecture

```
Cargo Workspace
├── crates/ladoo          — main framework crate (~32k LOC)
├── crates/ladoo-macros   — derive macros (Config, AppError, Job)
├── crates/ladoo-cli      — CLI tool (placeholder)
├── crates/ladoo-migrate  — standalone migration tool (~10k LOC)
└── examples/hello-world  — minimal example
```

Key decisions:
- **Runtime:** Tokio (hardcoded, not abstracted)
- **Server:** Hyper native with auto HTTP/1.1 and HTTP/2
- **Handlers:** `Box<dyn Handler>` — dynamic dispatch for fast compilation
- **State:** `Arc<T>` inside `TypeMap` — cheap extraction, no `Clone` bound
- **Middleware:** Plain async functions (not Tower)

## Progress

### Done

| Phase | Feature | Tests |
|-------|---------|-------|
| 1-4 | Workspace, Response/Request, Handler, Router, Extractors (Json, Query), Error system | ~100 |
| 5 | State & Dependency Injection (`TypeMap`, `State<T>`, `App::provide`) | ~30 |
| 6 | Middleware & Routing (trait, groups, mounting, context) | ~40 |
| 7 | Testing Utilities (`TestClient`, `TestServer`) | ~60 |
| 8 | Configuration (`Environment`, `Config` trait, `#[derive(Config)]`, TOML) | ~30 |
| 9 | Structured Logging (tracing, `RequestId`, request logger) | ~30 |
| 10 | Security Headers & Graceful Shutdown | ~20 |
| 10b | Wildcard Routes (`*path` catch-all) | 17 |
| 11 | Plugin System (`Plugin` trait, `ShutdownHook`) | ~15 |
| 12 | Validation (`Valid<T>`, `Validate` trait, `validator` integration) | ~30 |
| 13 | Auth System (`AuthProvider`, `Auth<T>`, `ApiKeyAuth`, `JwtAuth`, RBAC) | ~62 |
| 14 | CORS & Rate Limiting | ~40 |
| 15 | Database Migrations (`ladoo-migrate` crate, CLI, SQLite/Postgres/MySQL) | ~168 |
| 16 | Health Checks & Pagination | ~64 |
| 17 | Caching (`CacheStore`, `MemoryStore`, `Cache<T>`) | ~20 |
| 18 | Security Hardening (8 findings fixed) | 24 |
| 19 | Path Extractor (`Path<T>` with custom serde deserializer) | 22 |
| 20 | Job Queue Mode 1 (`Job` trait, `#[derive(Job)]`, `JobRunner`) | ~30 |
| 21 | HTTP/2 & TLS (h2c auto-detection, feature-gated rustls) | 12 |
| 22 | 405 Method Not Allowed (with `Allow` header) | 14 |

**Total: ~700+ tests across the workspace**

### Planned

- **Arc state optimization** — store `Arc<T>` in TypeMap, drop `Clone` bound from `State<T>` and `Auth<T>` (in progress)
- **Radix router** — replace linear route matching with a radix tree for O(path-length) lookups
- **Middleware chain reuse** — build the chain once at startup instead of allocating per-request
- **Body streaming** — opt-in streaming for large uploads (body limit is already in place)
- **Job queue modes 2-3** — database-backed queue and external (SQS/Redis) adapters
- **WebSockets** — built on hyper's upgrade support
- **Connection pooling** — trait-based, database-agnostic pool management
- **HTTP/3** — QUIC via quinn/h3 (waiting for ecosystem maturity)
- **OpenAPI** — auto-generated API documentation from route definitions
- **CI benchmarks** — automated performance regression testing

## Feature Flags

| Flag | Default | What it enables |
|------|---------|----------------|
| `json` | on | `Json<T>`, `Query<T>`, `Path<T>` extractors, pagination |
| `macros` | on | `#[derive(Config)]`, `#[derive(AppError)]`, `#[derive(Job)]` |
| `config` | on | TOML configuration loading |
| `logging` | on | Structured logging with tracing |
| `cache` | off | `Cache<T>` wrapper and `CacheStore` trait |
| `jobs` | off | `Job` trait, `JobRunner`, `#[derive(Job)]` |
| `auth-jwt` | off | `JwtAuth` provider (adds `jsonwebtoken` dependency) |
| `validation` | off | `validator` crate blanket impl for `Validate` |
| `tls` | off | HTTPS via rustls with ALPN HTTP/2 negotiation |

## License

MIT
