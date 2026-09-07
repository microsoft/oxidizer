<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Rallocator Telemetry Logo" width="96">

# Rallocator Telemetry

[![crate.io](https://img.shields.io/crates/v/rallocator_telemetry.svg)](https://crates.io/crates/rallocator_telemetry)
[![docs.rs](https://docs.rs/rallocator_telemetry/badge.svg)](https://docs.rs/rallocator_telemetry)
[![MSRV](https://img.shields.io/crates/msrv/rallocator_telemetry)](https://crates.io/crates/rallocator_telemetry)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Owned rallocator snapshot schema and binary encoding.

Snapshot data is organized into [`snapshot`][__link0], [`topology`][__link1], and
[`callers`][__link2]. The root exports encoding functions and their shared error.

## Compatibility contract

A snapshot has three layers with independent versions:

* `rallocator_wire` owns the little-endian container header and
  length-prefixed section framing. A framing change increments the wire
  version, and readers reject unknown wire versions.
* This crate owns the telemetry schema named by the header. A change that
  reinterprets the snapshot as a whole increments that schema; readers
  reject unsupported schema versions.
* Each section owns its payload version. Compatible extensions increment
  only that section version. Unknown sections and unsupported optional
  section versions are skipped and reported through
  [`snapshot::Snapshot::skipped_sections`][__link3].

Metadata and statistics sections are required. Historical section versions
accepted by the decoder receive documented neutral defaults for fields that
did not yet exist. Producers must not change the meaning or byte order of an
existing version.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/rallocator_telemetry">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbA9BBc3IJUEYbdz6zmazZCsMbR0-Fmqyx0SsbF2aVShPOGEdhZIGCdHJhbGxvY2F0b3JfdGVsZW1ldHJ5ZTAuMS4w
 [__link0]: https://docs.rs/rallocator_telemetry/0.1.0/rallocator_telemetry/snapshot/index.html
 [__link1]: https://docs.rs/rallocator_telemetry/0.1.0/rallocator_telemetry/topology/index.html
 [__link2]: https://docs.rs/rallocator_telemetry/0.1.0/rallocator_telemetry/callers/index.html
 [__link3]: https://docs.rs/rallocator_telemetry/0.1.0/rallocator_telemetry/?search=snapshot::Snapshot::skipped_sections
