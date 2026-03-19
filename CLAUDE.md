# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run Commands

- **Build:** `cargo build`
- **Run:** `cargo run` (serves on http://127.0.0.1:8080)
- **Check (fast compile check):** `cargo check`
- **Tests:** `cargo test` / single test: `cargo test <test_name>`
- **Clippy lint:** `cargo clippy`

## Prerequisites

The app requires running instances of:
- **PostgreSQL** on `localhost:5432` — database `rustdb`, user `postgres`, password `postgres`
- **Redis** on `127.0.0.1:6379` (default)

A `users` table must exist in PostgreSQL with columns `id INTEGER` and `name TEXT`.

## Architecture

Single-file Axum web server (`src/main.rs`) with a cache-aside pattern using Redis in front of PostgreSQL.

**Stack:** Axum 0.7 (async web framework), SQLx 0.7 (Postgres, async), redis 0.24 (async), Tokio runtime.

**AppState** (shared via `Arc`): holds `PgPool` and `redis::Client`, passed to handlers through Axum's `State` extractor.

**Routes:**
- `GET /` — HTML UI for testing the API
- `POST /users` — insert user into Postgres
- `GET /users` — list all users from Postgres
- `GET /users/:id` — get user by ID (Redis cache-first, falls back to Postgres, then caches result as Redis hash `user:<id>`)
- `GET /cache` — list all cached users from Redis
- `POST /cache/clear` — flush Redis DB

**Caching strategy:** Redis hashes keyed `user:<id>` with field `data` storing serialized JSON. On cache miss, the value is fetched from Postgres and written to Redis.

## Code Conventions

- Comments and UI text are in English
- Rust 2021 edition
