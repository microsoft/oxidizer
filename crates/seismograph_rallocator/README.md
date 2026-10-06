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

Native v4 allocator observations for Seismograph.

[`native::Snapshot`][__link0] inventories persistent owners, including never-observed
active endpoints, and an independently collected global backend. Owner state
is bounded and may be stale: compare session, lease generation and round.
Idle inspection is fresh under the pool lock; busy, unobserved and slot
allocation failure (`Unavailable`) are explicitly unknown. Native capacity is not application-live
memory, remote batching budget is not pending bytes, and globally cached
ranges are not guaranteed physically decommitted.
Matching rounds mean contributed this round, not an exact current census:
the first round can predate polling. Consumers show observation age at capture.
Outstanding allocator ranges include pending/retained frees, not app-live
object counts. Incoming front/back inequality means potential work only,
not guaranteed ready links or queue depth; equality does not prove emptiness.

[`encoded_len`][__link1], [`encode`][__link2] and [`decode`][__link3] implement schema 3 exclusively.
The decoder bounds allocation by both the payload length and [`MAX_OWNERS`][__link4],
rejects invalid flags, duplicates, inconsistent inventory and trailing bytes.
Application allocation events remain in the unchanged Seismograph container.
[`encoded_len_with_owners`][__link5] and [`encode_with_owners`][__link6] accept borrowed owner
rows, ignoring the metadata snapshot’s vector. Both use fixed stack scratch
and never allocate, so a producer can keep inventory and output System-backed
without native allocator activity perturbing the observations it collects.
[`events::callers`][__link7] projects these into view-local correlation identities,
preserving stacks, actor names, orphan frees and repeated addresses.

Self-publication defaults on, but recording defaults off. Polling requests
another observation round without a background thread or per-operation clock
check. Controls belong to this plugin, not the allocator’s public API.
Only contributing accepted recorded allocation/free operations publish, after
the native operation ends; sampled-out events and merely enabled attempts do
not contribute. Collectors request the next round after collection, and apps
may use [`native::request_observation`][__link8] to schedule requests explicitly.

## Explore a sample capture

```text
cargo +1.95.0 run -p seismograph_rallocator --example native_snapshot
cargo +1.95.0 run -p seismograph_cli -- view native-demo.seismograph
cargo +1.95.0 run -p seismograph_cli -- snapshot html native-demo.seismograph
```

The example uses synthetic native state and real recorder events, including
address reuse and an orphan free. It does not install the native allocator.
Its capture callback encodes borrowed stack rows into System-backed `SourceData`.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_rallocator">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbAXJgUeSXjZMbZaYTfsaEXvQbE_CfSnnGi2AbpPgIGakgPdthZIGCdnNlaXNtb2dyYXBoX3JhbGxvY2F0b3JlMC4xLjA
 [__link0]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/?search=native::Snapshot
 [__link1]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/?search=encoded_len
 [__link2]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/?search=encode
 [__link3]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/?search=decode
 [__link4]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/?search=MAX_OWNERS
 [__link5]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/?search=encoded_len_with_owners
 [__link6]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/?search=encode_with_owners
 [__link7]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/?search=events::callers
 [__link8]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/?search=native::request_observation
