<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Msvc Spectre Libs Logo" width="96">

# Msvc Spectre Libs

[![crate.io](https://img.shields.io/crates/v/msvc_spectre_libs.svg)](https://crates.io/crates/msvc_spectre_libs)
[![docs.rs](https://docs.rs/msvc_spectre_libs/badge.svg)](https://docs.rs/msvc_spectre_libs)
[![MSRV](https://img.shields.io/crates/msrv/msvc_spectre_libs)](https://crates.io/crates/msvc_spectre_libs)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Links the Spectre-mitigated C runtime libraries for Windows MSVC targets.

Add this crate as a dependency to link its consumers with the
[Spectre-mitigated libraries][__link0] installed by Visual Studio. The crate has no
runtime API and does nothing for non-Windows or non-MSVC targets.

Visual Studio Installer offers the libraries as optional components such as
`MSVC v143 - VS 2022 C++ x64/x86 Spectre-mitigated libs (Latest)`.

## Missing libraries

By default, the build script emits a warning and continues when it cannot
find the libraries. Enable the `error` feature to fail the build instead.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/msvc_spectre_libs">source code</a>.
</sub>

 [__link0]: https://learn.microsoft.com/cpp/build/reference/qspectre
