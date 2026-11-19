<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Seismograph Rallocator Logo" width="96">

# Seismograph Rallocator

[![crate.io](https://img.shields.io/crates/v/seismograph_rallocator.svg)](https://crates.io/crates/seismograph_rallocator)
[![docs.rs](https://docs.rs/seismograph_rallocator/badge.svg)](https://docs.rs/seismograph_rallocator)
[![MSRV](https://img.shields.io/crates/msrv/seismograph_rallocator)](https://crates.io/crates/seismograph_rallocator)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Backend-aware allocator observations for Seismograph.

[`snapshot::Snapshot`][__link0] combines a shared observation header with typed
[`snmalloc`][__link1] or [`mimalloc`][__link2] state. Its schema-5 codec reads both backends.
Other schemas are rejected. Allocation events remain independent of
backend state in the surrounding Seismograph container.

Mimalloc state includes bounded page summaries from owner threads and
independently sampled arena, abandoned-page, and direct-OS state. Missing,
busy, failed, and exited owners remain explicit. Used blocks can include
uncollected cross-thread frees. Committed bytes are not physical residency.
Direct-OS commitment covers data only, not metadata.

The snmalloc model, [`native::Snapshot`][__link3], inventories persistent owners, including never-observed
active endpoints, and an independently collected global backend. Owner state
is bounded and can be stale: compare lease generation, round, and capture time.
Idle inspection is fresh under the pool lock; busy, unobserved and slot
allocation failure (`Unavailable`) are explicitly unknown. Native capacity is not application-live
memory, remote batching budget is not pending bytes, and globally cached
ranges are not guaranteed physically decommitted.
Matching rounds mean contributed this round, not an exact current census:
the first round can predate polling. Consumers show observation age at capture.
Outstanding allocator ranges include pending/retained frees, not app-live
object counts. Incoming front/back inequality means potential work only,
not guaranteed ready links or queue depth; equality does not prove emptiness.

[`encoded_len`][__link4], [`encode`][__link5] and [`decode`][__link6] handle snmalloc in the current schema.
Use [`snapshot`][__link7] to encode or decode either backend.
The decoder bounds allocation by both the payload length and [`MAX_OWNERS`][__link8],
rejects invalid flags, duplicates, inconsistent inventory and trailing bytes.
Application allocation, resize, and free events remain in the Seismograph container.
[`encoded_len_with_owners`][__link9] and [`encode_with_owners`][__link10] accept borrowed owner
rows, ignoring the metadata snapshot’s vector. Both use fixed stack scratch
and never allocate, so a producer can keep inventory and output System-backed
without native allocator activity perturbing the observations it collects.
[`events::callers`][__link11] projects these into view-local correlation identities,
preserving stacks, actor names, orphan records and repeated addresses.
Successful resizes keep the original view-local lifetime across address changes.
Allocation counts describe original allocations. Unmatched size summaries use
the last retained resize. Missing events can still hide lifetime endpoints.

Self-publication defaults on, but recording defaults off. Polling requests
another observation round without a background thread or per-operation clock
check. Controls belong to this plugin, not the allocator’s public API.
Operations with an enabled allocation policy publish after the native operation
ends, at most once per lease and requested round. Event sampling does not
control publication. Collectors request the next round after collection.
Applications can also call [`native::request_observation`][__link12] explicitly.

## Explore a sample capture

```text
cargo +1.95.0 run -p seismograph_rallocator --example native_snapshot
cargo +1.95.0 run -p seismograph_cli -- view native-demo.seismograph
```

The example uses synthetic native state and real recorder events, including
address reuse and an orphan free. It does not install the native allocator.
Its capture callback encodes borrowed stack rows into System-backed `SourceData`.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_rallocator">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbw7Ld7e-ppCEbfBTJFts6MhkbazYIyWwJQfkbRlzqWLynR3FhZIGCdnNlaXNtb2dyYXBoX3JhbGxvY2F0b3JlMC41LjA
 [__link0]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/?search=snapshot::Snapshot
 [__link1]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/snmalloc/index.html
 [__link10]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/?search=encode_with_owners
 [__link11]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/?search=events::callers
 [__link12]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/?search=native::request_observation
 [__link2]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/mimalloc/index.html
 [__link3]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/?search=native::Snapshot
 [__link4]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/?search=encoded_len
 [__link5]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/?search=encode
 [__link6]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/?search=decode
 [__link7]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/snapshot/index.html
 [__link8]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/?search=MAX_OWNERS
 [__link9]: https://docs.rs/seismograph_rallocator/0.5.0/seismograph_rallocator/?search=encoded_len_with_owners
