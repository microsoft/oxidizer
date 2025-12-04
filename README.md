<div align="center">
 <img src="./logo.svg" alt="Oxidizer Logo" width="128" height="128">

# The Oxidizer Project

[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

</div>

This repository contains a set of crates that help you build robust highly scalable services in Rust.

- [Crates](#crates)
- [Repo Guidelines](#repo-guidelines)
- [Building This Repo](#building-this-repo)
- [Generating Documentation](#generating-documentation)
- [Releasing Crate Versions](#releasing-crate-versions)
- [Trademarks](#trademarks)

## Crates

These are the crates built out of this repo:

- [`bytesbuf`](./crates/bytesbuf/README.md) - Manipulate sequences of bytes for efficient I/O.
- [`data_privacy`](./crates/data_privacy/README.md) - Mechanisms to classify, manipulate, and redact sensitive data.
- [`data_privacy_macros`](./crates/data_privacy_macros/README.md) - Macros for the `data_privacy` crate.
- [`fundle`](./crates/fundle/README.md) - Compile-time safe dependency injection for Rust.
- [`fundle_macros`](crates/fundle_macros/README.md) - Macros for the `fundle` crate.
- [`fundle_macros_impl`](crates/fundle_macros_impl/README.md) - Macros for the `fundle` crate.
- [`ohno`](./crates/ohno/README.md) - High-quality Rust error handling.
- [`ohno_macros`](./crates/ohno_macros/README.md) - Macros for the `ohno` crate.
- [`thread_aware`](./crates/thread_aware/README.md) - Facilities to support thread-isolated state.
- [`thread_aware_macros`](./crates/thread_aware_macros/README.md) - Macros for the `thread_aware` crate.
- [`thread_aware_macros_impl`](./crates/thread_aware_macros_impl/README.md) - Macros for the `thread_aware` crate.

## Repo Guidelines

- Every PR submitted to this repo must follow the [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) specification.

- Every crate built in this repo should:
  - Have an entry in the Crates section above.
  - Have an entry in [CHANGELOG.md](./CHANGELOG.md).
  - Have a README.md file generated using [`cargo-rdme`](https://docs.rs/cargo-rdme/latest/cargo_rdme/)
    with a consistent set of badges (see [crates/data_privacy/README.md](./crates/data_privacy/README.md) as an example)
  - Have a CHANGELOG.md file generated using the `release-crate.ps1` script.
  - Have a meaningful set of categories and keywords in their Cargo.toml file (see
    [crates/data_privacy/Cargo.toml](./crates/data_privacy/Cargo.toml) as an example).
    The `oxidizer` keyword should always be present.
  - Have a Rust-inspired logo and favicon.

The best way to get started with a new crate is to run `scripts\add-crate.ps1` which will create a new folder
and populate it to get you started on a new crate.

## Building This Repo

We use standard Rust tooling to build, test, lint, and format our source base. In addition, we rely on these
tools, which you may wish to install:

- [`cargo-audit`](https://crates.io/crates/cargo-audit) - used to ensure we don't use known-bad crate versions.

- [`cargo-check-external-types`](https://crates.io/crates/cargo-check-external-types) - used to control the types we expose

- [`cargo-hack`](https://crates.io/crates/cargo-hack) - used to exhaustively run tests against multiple features.

- [`cargo miri`](https://doc.rust-lang.org/cargo/commands/cargo-miri.html) - used to statically verify unsafe code sequences.

- [`cargo-mutants`](https://crates.io/crates/cargo-mutants) - used to perform mutation testing

- [`cargo-rdme`](https://crates.io/crates/cargo-rdme) - generates README files based on a crate's top-level docs.

- [`cargo-udeps`](https://crates.io/crates/cargo-udeps) - used to ensure crates don't declare superfluous dependencies.

## Generating Documentation

To generate documentation locally with all features enabled (including feature-gated items), run:

```powershell
.\scripts\generate-docs.ps1
```

This requires the Rust nightly toolchain to be installed. The script will generate documentation
and open it in your default browser.

## Releasing Crate Versions

Releasing new versions of crates to [crates.io](https://crates.io) is handled by
an internal Microsoft automation process. To release a new version of any crate, use
the `scripts\release-crate.ps1` script. For example:

1. Make the changes you'd like to release and commit them to the repo. Don't push them
to GitHub, just commit them:

    ```bash
    git add .
    git commit -m "feat: Add the GoFast feature"
    ```

1. Run the release script, supplying the name of the crate you want to release. The script will
update the version number in a few places and update the appropriate `CHANGELOG` file:

    ```bash
    .\scripts\release-crate.ps1 <crate_name>
    ```

1. Amend the newly edited files back into your commit

    ```bash
    git add .
    git commit --amend --no-edit
    ```

1. Create a PR like normal.

Once your PR is merged, automation will kick in
to tag the commit and push the crate to crates.io.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft
trademarks or logos is subject to and must follow
[Microsoft's Trademark & Brand Guidelines](https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship.
Any use of third-party trademarks or logos are subject to those third-party's policies.
