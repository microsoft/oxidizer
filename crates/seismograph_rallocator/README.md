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

Rallocator snapshot source for seismograph.

Rallocator contributes this payload to the process-wide [`seismograph`][__link0]
snapshot. Snapshot data is organized into [`snapshot`][__link1], [`topology`][__link2], and
[`callers`][__link3].

## Compatibility contract

A snapshot has three layers with independent versions:

* The private wire layer owns the little-endian container header and
  length-prefixed section framing. A framing change increments the wire
  version, and readers reject unknown wire versions.
* This crate owns the telemetry schema named by the header. A change that
  reinterprets the snapshot as a whole increments that schema; readers
  reject unsupported schema versions.
* Each section owns its payload version. Compatible extensions increment
  only that section version. Unknown sections and unsupported optional
  section versions are skipped and reported through
  [`snapshot::Snapshot::skipped_sections`][__link4].

Metadata and statistics sections are required. Statistics must use the current
section version; legacy statistics payloads are not supported. Producers must
not change the meaning or byte order of an existing version.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_rallocator">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbEcLzBLld0bkb1MkXB7BgGAUb_SG7uw5pXb4bEk90SYhdPJVhZIKCa3NlaXNtb2dyYXBoZTAuMS4wgnZzZWlzbW9ncmFwaF9yYWxsb2NhdG9yZTAuMS4w
 [__link0]: https://crates.io/crates/seismograph/0.1.0
 [__link1]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/snapshot/index.html
 [__link2]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/topology/index.html
 [__link3]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/callers/index.html
 [__link4]: https://docs.rs/seismograph_rallocator/0.1.0/seismograph_rallocator/?search=snapshot::Snapshot::skipped_sections
