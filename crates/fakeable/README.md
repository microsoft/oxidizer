<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Fakeable Logo" width="96">

# Fakeable

[![crate.io](https://img.shields.io/crates/v/fakeable.svg)](https://crates.io/crates/fakeable)
[![docs.rs](https://docs.rs/fakeable/badge.svg)](https://docs.rs/fakeable)
[![MSRV](https://img.shields.io/crates/msrv/fakeable)](https://crates.io/crates/fakeable)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Proc macros for streamlining the creation of fakeable structures.

This crate provides the `fakeable` attribute macro that can be applied to structs
and their impl blocks to automatically generate a wrapper structure that can switch
between real and fake implementations at runtime. The fake implementation is only
compiled when the specified feature flag (default: “test-util”) is enabled or during
test builds.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fakeable">source code</a>.
</sub>

