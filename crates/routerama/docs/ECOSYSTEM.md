# Routerama in the Rust routing ecosystem

How `routerama` compares to widely used Rust path-routing crates. The benchmark
suite includes `matchit`, `path-tree`, and `route-recognizer` as
dev-dependencies at the versions resolved in `Cargo.lock` (the manifests use
caret ranges, so compatible updates may move these):

- [`matchit`](https://crates.io/crates/matchit) 0.8.4 — the router inside **axum**.
- [`path-tree`](https://crates.io/crates/path-tree) 0.8.3 — the router inside **viz**.
- [`route-recognizer`](https://crates.io/crates/route-recognizer) 0.3.1 — used by the legacy **tide** stack.

The feature comparison below also includes
[`actix-router`](https://crates.io/crates/actix-router) 0.5.4, the router inside
**actix-web**. It is intentionally not a `routerama` dev-dependency or benchmark
target because its unconditional `tracing/log` feature changes workspace-wide
coverage instrumentation for unrelated telemetry call sites.

Feature support is nuanced and evolves between releases (e.g. `matchit` 0.9
changed its wildcard syntax), so treat this as a snapshot of the compared and
resolved versions rather than a permanent statement.

## What makes routerama different

Every other crate here is a **runtime-only** router: you build a router *value*
and insert routes into it at startup. `routerama`'s headline feature is that the
same route set can instead be lowered to a **static router at compile time** — via
`#[resolver]` — producing an
exhaustive, `match`-able `enum` with captures already coerced to typed fields and
*no* router value to construct or keep alive. When the routes are only known at
run time, the generated builder registers dynamic variants with the same typed
`enum` result. One resolver may contain both static and dynamic variants.

Three further differences fall out of that design:

- **Method + custom-verb dispatch is built in.** `resolve(method, path)` matches
  the HTTP method, and a trailing `:verb` (`google.api.http` custom verbs, e.g.
  `/books/{book}:archive`) is part of the template. The other four crates are
  **path-only**: the caller selects on the method separately.
- **The result is a typed route**, not a string-keyed parameter map. The static
  backend hands back an `enum` variant you can `match` exhaustively, with each
  capture already bound to a named field. The flip side: unlike the rest of the
  ecosystem, routerama does *not* store an arbitrary value/handler per route and
  return it on match (the `Router<T>` model that `matchit`, `path-tree`,
  `actix-router`, and `route-recognizer` all use, and that axum
  layers handler dispatch onto). routerama returns the route *identity*; you own
  the mapping from identity to behavior (a `match` for the static backend, or a
  lookup keyed on the route name for the dynamic one).
- **Static captures can borrow the request path** and matching allocates
  nothing until a field explicitly requests owned decoding. Dynamic variants
  own their captures; their raw matching metadata stays on the stack for up to
  4 captures / 16 segments.

The main thing `routerama` does *not* offer that one competitor does:
**per-segment regex/type constraints** (`actix-router`'s `{id:\d+}`). `routerama`
uses `google.api.http` path templates, which have no regex escape hatch.

## Comparison

| | **routerama** | matchit 0.8.4 | path-tree 0.8.3 | actix-router 0.5.4 | route-recognizer 0.3.1 |
|---|---|---|---|---|---|
| **Compile-time static router** | **Yes** (codegen) | No | No | No | No |
| **Runtime router** | Yes | Yes | Yes | Yes | Yes |
| **Static/dynamic hybrid** | **Yes** (fallback) | No | No | No | No |
| **HTTP method / custom-verb dispatch** | **Built in** | No (path-only) | No (path-only) | No (path-only) | No (path-only) |
| **Result is a typed route** | **Yes** (`enum` variant) | No (param map) | No (param map) | No (param map) | No (param map) |
| **Store a value / handler per route (`Router<T>`)** | **No** (identity only) | Yes | Yes | Yes | Yes |
| **Catch-all (capture rest of path)** | Yes | Yes | Yes | Yes | Yes |
| **Mid-segment (affix) capture** | Yes | 1 param/segment | Many params/segment | Yes | Dot-separated only |
| **Per-segment regex / type constraints** | No | No | No | **Yes** | No |
| **Optional segments** | No | No | **Yes** | No | No |
| **Captures borrow the path (no copy)** | Yes | Yes | Yes | Yes | No (owned `String`) |
| **`no_std`** | **Yes** (`alloc`) | No | **Yes** | No | No |
| **Maintained** | Yes | Yes | Yes | Yes | **No** (since 2021) |

## When to reach for another crate

`routerama` is aimed at services whose route set is known at build time and want
the tightest possible dispatch plus a typed result. Prefer a different crate
when:

- **You need per-segment regex or type constraints** — `actix-router`
  (`/user/{id:\d+}`) is the only option here that supports them.
- **You need `#![forbid(unsafe_code)]`** — `path-tree` forbids unsafe code,
  whereas `routerama`'s fast scan path uses `unsafe` (SIMD plus unchecked
  slicing). Both `routerama` and `path-tree` are `#![no_std]` (each needs
  `alloc`), so either works without `std`.
- **You are already inside a framework** — if you use axum, actix-web, or viz,
  that framework's built-in router (`matchit`, `actix-router`, `path-tree`
  respectively) is the path of least resistance.
- **You want the smallest possible dependency for purely runtime routing** —
  `matchit` is a tiny, dependency-free radix trie and is extremely fast for a
  runtime router.

## Performance

`routerama` maintains apples-to-apples benchmarks against `matchit`, `path-tree`,
and `route-recognizer`, plus a regex-based baseline — same route table, same
typed end state (method validated and every capture converted to its declared
`&str`, `u32`, or `String` type) — in
[`PERF.md`](PERF.md) (regenerated by
`scripts/perf_report.rs`). See that report for current numbers; the harness
lives in [`benches/common/`](../benches/common/). Because the third-party
routers only *select* a route, the harness explicitly does the method check and
parameter extraction afterwards so all routers are driven to the same result.
