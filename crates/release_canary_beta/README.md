<div align="center">
 <img src="./logo.png" alt="Release Canary Beta Logo" width="96">

# Release Canary Beta

[![crate.io](https://img.shields.io/crates/v/release_canary_beta.svg)](https://crates.io/crates/release_canary_beta)
[![docs.rs](https://docs.rs/release_canary_beta/badge.svg)](https://docs.rs/release_canary_beta)
[![MSRV](https://img.shields.io/crates/msrv/release_canary_beta)](https://crates.io/crates/release_canary_beta)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Canary crate used to exercise the release pipeline.

This crate exists solely as a low-risk publish target so that changes to the release
infrastructure can be validated end-to-end without touching production crates. It exposes
a single function that returns a constant identifier; the value is intentionally trivial
so that the crate’s behavior never needs to change between releases.

## Examples

```rust
assert_eq!(release_canary_beta::canary_name(), "beta");
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/release_canary_beta">source code</a>.
</sub>

