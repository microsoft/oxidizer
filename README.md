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
- [Releasing Crate Versions](#releasing-crate-versions)
- [Trademarks](#trademarks)

## Crates

These are the crates built out of this repo:

- [`data_privacy`](./crates/data_privacy/README.md) - Mechanisms to classify, manipulate, and redact sensitive data.
- [`data_privacy_macros`](./crates/data_privacy_macros/README.md) - Macros to generate data taxonomies.

## Repo Guidelines

- Every PR submitted to this repo should follow the [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) specification.

- Every crate built in this repo should:
  - Have an entry in the Crates section above.
  - Have an entry in [CHANGELOG.md](./CHANGELOG.md).
  - Have a README.md file generated using [`cargo-rdme`](https://docs.rs/cargo-rdme/latest/cargo_rdme/)
    with a consistent set of badges (see [crates/data_privacy/README.md](./crates/data_privacy/README.md) as an example)
  - Have a CHANGELOG.md file generated using [`git-cliff`](https://git-cliff.org/docs/).
  - Have a meaningful set of categories and keywords in their Cargo.toml file (see
    [crates/data_privacy/Cargo.toml](./crates/data_privacy/Cargo.toml) as an example).
    The `oxidizer` keyword should always be present.
  - Have a Rust-inspired logo.

## Releasing Crate Versions

Releasing new versions of crates to [crates.io](https://crates.io) is handled by
an internal Microsoft process. To release a new version of any crate,
bump the version in accordance to semver using the `cargo set-version` command available
via the [`cargo-edit`](https://crates.io/crates/cargo-edit) crate.
For example:

```bash
cargo set-version -p data_privacy_macros -p data_privacy --bump minor
```

After the version bump is checked in, an automated process will publish the
updated crates within 48 hours.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft
trademarks or logos is subject to and must follow
[Microsoft's Trademark & Brand Guidelines](https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship.
Any use of third-party trademarks or logos are subject to those third-party's policies.
