<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Seismograph Cli Logo" width="96">

# Seismograph Cli

[![crate.io](https://img.shields.io/crates/v/seismograph_cli.svg)](https://crates.io/crates/seismograph_cli)
[![docs.rs](https://docs.rs/seismograph_cli/badge.svg)](https://docs.rs/seismograph_cli)
[![MSRV](https://img.shields.io/crates/msrv/seismograph_cli)](https://crates.io/crates/seismograph_cli)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Live monitoring and snapshot-to-HTML reporting for the `seismograph` command.

The binary’s entry point parses arguments with [`clap`][__link0], following the same
shape as this crate’s own (private) `Cli` type:

```rust
use clap::Parser;

#[derive(Parser)]
#[command(name = "seismograph")]
struct Cli {
    /// Path to a snapshot file to render.
    path: std::path::PathBuf,
}

let cli = Cli::parse_from(["seismograph", "snapshot.bin"]);
assert_eq!(cli.path, std::path::PathBuf::from("snapshot.bin"));
```

The CLI renders common thread, stack, and runtime-event data directly.
Rallocator payloads use the built-in schema-specific renderer; unknown
sources remain visible in the source inventory.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_cli">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbAcUP8elXFk0bF_C0i6BnyRIbN5eqLuarxUIbr20fkg-Cc05hZIGCZGNsYXBlNC42LjY
 [__link0]: https://crates.io/crates/clap/4.6.6
