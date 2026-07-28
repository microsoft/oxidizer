<div align="center">

# Fetch WinHTTP

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

WinHTTP-based HTTP transport for `fetch` (Windows only).

## Internal implementation detail

This crate is an internal implementation detail of the SDK. It is not part
of the public API surface, must not be re-exported, and offers no stability
guarantees: anything may change in any release, including patch releases.

## Status: skeleton

The transport is **not implemented yet**. This crate currently contains only
the scaffolding and the design documentation under [`docs/design/`](docs/design).
Those documents define the intended API shape, how the transport plugs into the
`fetch` pipeline as a `RequestHandler`, the strategy for bridging WinHTTP's
asynchronous completion callbacks to Rust futures, and the configuration mapping
between `fetch_options`/`fetch_tls` and WinHTTP (including the knobs that do not
map cleanly).

> **Note:** This README is normally auto-generated from `lib.rs` via
> `just readme`. It was seeded by hand for the initial skeleton and will be
> regenerated once the crate has public API to document.

<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_winhttp">source code</a>.
</sub>
