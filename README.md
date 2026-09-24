<div align="center">
 <img src="./logo.svg" alt="Oxidizer Logo" width="96">

# The Oxidizer Project

[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

</div>

This repository contains a set of crates that help you build robust highly scalable services in Rust.

- [The Oxidizer Project](#the-oxidizer-project)
  - [Crates](#crates)
  - [About this Repo](#about-this-repo)
    - [Adding New Crates](#adding-new-crates)
    - [Publishing Crates](#publishing-crates)
    - [Documenting Crates](#documenting-crates)
    - [CI Workflows](#ci-workflows)
    - [Pull Request Gates](#pull-request-gates)
    - [Local Verification](#local-verification)
  - [Trademarks](#trademarks)

## Crates

These are the primary crates built out of this repo:

- [`anyspawn`](./crates/anyspawn/README.md) - A generic task spawner compatible with any async runtime.
- [`anyspawn_azure`](./crates/anyspawn_azure/README.md) - Azure SDK async runtime and process executor backed by an anyspawn spawner and a tick clock.
- [`allocation_hints`](./crates/allocation_hints/README.md) - Allocator-independent heap ownership and scoped allocation hints.
- [`arty`](./crates/arty/README.md) - Single-threaded, thread-aware application runtime.
- [`bytesbuf`](./crates/bytesbuf/README.md) - Types for creating and manipulating byte sequences.
- [`bytesbuf_io`](./crates/bytesbuf_io/README.md) - Asynchronous I/O abstractions expressed via `bytesbuf` types.
- [`cachet`](./crates/cachet/README.md) - A composable, customizable multi-tier caching library with rich feature support.
- [`cachet_memory`](./crates/cachet_memory/README.md) - In-memory cache tier backed by Moka for the cachet caching library.
- [`cachet_service`](./crates/cachet_service/README.md) - Layered service integration for the cachet caching library.
- [`cachet_tier`](./crates/cachet_tier/README.md) - Core cache tier trait and abstractions for building cache backends.
- [`compressors`](./crates/compressors/README.md) - Streaming compression and decompression over bytesbuf byte sequences.
- [`data_privacy`](./crates/data_privacy/README.md) - Mechanisms to classify, manipulate, and redact sensitive data.
- [`fetch`](./crates/fetch/README.md) - "Universal, composable and resilient HTTP client."
- [`fetch_azure`](./crates/fetch_azure/README.md) - Azure SDK HTTP transport backed by the fetch HTTP client.
- [`fetch_hyper`](./crates/fetch_hyper/README.md) - Hyper-based HTTP transport utilities for fetch.
- [`fetch_options`](./crates/fetch_options/README.md) - Options types for 'fetch' crate.
- [`fetch_winhttp`](./crates/fetch_winhttp/README.md) - WinHTTP-based HTTP transport for the fetch client (Windows only).
- [`fundle`](./crates/fundle/README.md) - Compile-time safe dependency injection for Rust.
- [`http_compression`](./crates/http_compression/README.md) - HTTP request and response body compression and decompression.
- [`http_extensions`](./crates/http_extensions/README.md) - Shared HTTP types and extension traits for clients and servers.
- [`http_headers`](./crates/http_headers/README.md) - Fast, ergonomic typed HTTP headers with borrowed views.
- [`http_path_template`](./crates/http_path_template/README.md) - Parser for the google.api.http path-template grammar.
- [`internity`](./crates/internity/README.md) - Blazingly fast string interning with compact handles, compact storage, and concurrent fill support.
- [`layered`](./crates/layered/README.md) - A foundational service abstraction for building composable, middleware-driven systems.
- [`metabench`](./crates/metabench/README.md) - Run Criterion, Gungraun, Linux perf, Intel VTune, and allocation benchmarks together and combine their reports.
- [`multitude`](./crates/multitude/README.md) - Fast and flexible arena allocator.
- [`ohno`](./crates/ohno/README.md) - High-quality Rust error handling.
- [`performables`](./crates/performables/README.md) - Thread-aware synchronization and ownership primitives.
- [`plurality`](./crates/plurality/README.md) - A highly efficient pooling memory allocator.
- [`rallocator`](./crates/rallocator/README.md) - A high-performance global allocator with passive allocation hints and telemetry.
  - [Supported platforms](./crates/rallocator/README.md#supported-platforms)
  - [Design guide](./crates/rallocator/README.md#design-guide)
  - [Implementation guide](./crates/rallocator/README.md#implementation-guide)
- [`seismograph_cli`](./crates/seismograph_cli/README.md) - Live monitoring and snapshot tools for seismograph telemetry.
- [`seismograph`](./crates/seismograph/README.md) - High-performance process telemetry with extensible snapshot sources.
- [`seismograph_io`](./crates/seismograph_io/README.md) - Structured I/O event instrumentation for Seismograph.
- [`seismograph_protocol`](./crates/seismograph_protocol/README.md) - Local monitor protocol and discovery model for Seismograph.
- [`seismograph_rallocator`](./crates/seismograph_rallocator/README.md) - Rallocator snapshot source for seismograph.
- [`seismograph_runtime`](./crates/seismograph_runtime/README.md) - Runtime and task lifecycle instrumentation for Seismograph.
- [`recoverable`](./crates/recoverable/README.md) - Recovery information and classification for resilience patterns.
- [`rest_over_grpc`](./crates/rest_over_grpc/README.md) - Automatically transcode gRPC services to REST/JSON endpoints.
- [`routerama`](./crates/routerama/README.md) - Blazingly fast HTTP route resolution and query string processing.
- [`seatbelt`](./crates/seatbelt/README.md) - Resilience and recovery mechanisms for fallible operations.
- [`seatbelt_http`](./crates/seatbelt_http/README.md) - HTTP-specific extensions for the seatbelt crate.
- [`templated_uri`](./crates/templated_uri/README.md) - Standards-compliant URI handling with templating, safety validation, and data classification
- [`thread_aware`](./crates/thread_aware/README.md) - Facilities to support thread-isolated state.
- [`thread_aware_core`](./crates/thread_aware_core/README.md) - Stable core traits and types for thread-aware state.
- [`tick`](./crates/tick/README.md) - Provides primitives to interact with and manipulate machine time.
- [`uniflight`](./crates/uniflight/README.md) - Coalesces duplicate async tasks into a single execution.

## About this Repo

The following sections explain the overall engineering process we use
in this repo.

To set up a local PC environment capable of exercising all the tooling used by this repo's development processes,
you can follow the guide in [DEVELOPMENT.md](./DEVELOPMENT.md).

### Adding New Crates

Adding a new crate to this repo is done by running the `scripts\add-crate.ps1` script.
It will prompt you for a few bits of state, and then will get everything wired up that
needs to be.

The `add-crate` script does the following:

- Adds an entry for the crate to the [Crates](#crates) section in this README file.

- Adds an entry for the crate to the top-level [CHANGELOG.md](./CHANGELOG.md) file.

- Prepares a `README.md` file for the crate, setup for use with [
  `cargo-doc2readme`](https://crates.io/crates/cargo-doc2readme)
  with a set of appropriate CI badges.

- Creates an empty `CHANGELOG.md` file for the crate, which will later get populated by the `scripts\release-packages.ps1`
  script.

- Creates placeholder `logo.png` and `favicon.ico` files for the crate, which you're expected to replace with legit
  crab-themed
  logo and icon.

### Publishing Crates

Releasing new versions of crates to [crates.io](https://crates.io) is handled by
an internal Microsoft automation process. To release a new version of any crate, follow
this simple process:

1. Make sure the changes you want to release have all been committed to the repo.

2. Create a branch off of main.

3. Run `./scripts/release-packages.ps1 -Packages '<crate_name>@<change_type>'` to update versions and changelogs.
   The change type for each package is one of `breaking`, `nonbreaking`, `patch`, or an explicit version like
   `1.0.0`. To release several crates together, list them all in the same `-Packages` argument
   (for example, `'foo@nonbreaking','bar@patch'`); the script plans the entire release up-front.

4. Create a PR like normal to push changes out.

Once your PR is merged, automation will kick in. It will tag the
commit and push the crate to crates.io.

### Documenting Crates

We want our crates to have world-class documentation such that our customers can enjoy discovering and using our
features. We expect our Rust code to be fully documented in the normal Rust way, and we introduce two doc-related
automation processes:

- The `README.md` file in each crate's directory is auto-generated from the crate-level documentation.
  We use the [`cargo-doc2readme`](https://crates.io/crates/cargo-doc2readme) tool which reads the crate docs, resolves intra-doc links, and
  generates the `README.md` file using a shared template. A pull request gate ensures the `README.md` file
  always reflects the latest crate documentation.

- The `CHANGELOG.md` file in each crate's directory is auto-generated from the commits to a crate's directory by the
  `scripts/release-packages.ps1` script.

To generate documentation locally with all features enabled (including feature-gated items), run:

```shell
just anvil-doc-build --open
```

Anvil uses the repository's selected stable toolchain for this command. The
script generates documentation and opens it in your default browser.

### CI Workflows

Cargo Anvil owns Rust verification:

- `anvil-pr.yml` runs impact-scoped pull request and merge-group checks across
  the supported operating-system and architecture matrix.
- `anvil-scheduled.yml` runs full-workspace backstops and longer checks.

Both workflows invoke the same generated `just anvil-*` recipes developers use
locally. Codecov reports remain available for detailed inspection, while the
Anvil coverage gate is authoritative.

### Pull Request Gates

The Anvil pull request tier covers formatting, linting, manifest policy,
documentation, dependency policy, SemVer analysis, external-type exposure,
tests, coverage, examples, MSRV compatibility, Miri, cargo-careful, Loom,
Bolero, and mutation testing. Run the complete tier locally with:

```shell
just anvil-pr
```

### Local Verification

Run `just anvil-setup` once to install the toolchains and tools selected by the
Anvil catalog. Useful focused commands include:

```shell
just anvil-build
just anvil-clippy
just anvil-fmt --fix
just anvil-readme --fix
just anvil-spellcheck
just anvil-pr-fast
```

Tool versions are generated in `justfiles/anvil/versions.just` and updated with
Cargo Anvil. Do not maintain a second tool-version list.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft
trademarks or logos is subject to and must follow
[Microsoft's Trademark & Brand Guidelines](https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft
sponsorship.
Any use of third-party trademarks or logos are subject to those third-party's policies.
