# The Oxidizer Project

[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

This repository contains a set of libraries that help you build robust highly scalable services in Rust.

## Crates

- [`data_privacy`](./crates/data_privacy/README.md)
- [`data_privacy_macros`](./crates/data_privacy_macros/README.md)

## Repo Guidelines

- Every PR submitted to this repo should follow the [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) specification.
- Every crate built in this repo should:
  - Have an entry in the Crates section above.
  - Have an entry in in [CHANGELOG.md](./CHANGELOG.md).
  - Have a README.md file generated using [`cargo-rdme`](https://docs.rs/cargo-rdme/latest/cargo_rdme/) with a consistent set of badges (see [crates/data_privacy/README.md](./crates/data_privacy/README.md) as an example)
  - Have a CHANGELOG.md file generated using [`git-cliff`](https://git-cliff.org/docs/).
  - Have a meaningful set of categories and keywords in their Cargo.toml file (see [crates/data_privacy/Cargo.toml](./crates/data_privacy/Cargo.toml) as an example). The `oxidizer` keyword should always be present.

## Contributing

This project welcomes contributions and suggestions. Most contributions require you to
agree to a Contributor License Agreement (CLA) declaring that you have the right to,
and actually do, grant us the rights to use your contribution. For details, visit
https://cla.microsoft.com.

When you submit a pull request, a CLA-bot will automatically determine whether you need
to provide a CLA and decorate the PR appropriately (e.g., label, comment). Simply follow the
instructions provided by the bot. You will only need to do this once across all repositories using our CLA.

This project has adopted the [Microsoft Open Source Code of Conduct](https://opensource.microsoft.com/codeofconduct/).
For more information see the [Code of Conduct FAQ](https://opensource.microsoft.com/codeofconduct/faq/)
or contact [opencode@microsoft.com](mailto:opencode@microsoft.com) with any additional questions or comments.

## Releases

Releases to crates.io are handled by an internal process. To release a new version of any of the crates,
bump the version in accordance to semver. You should use cargo set-version for this to ensure all appropriate
files are updated. For example:

```bash
cargo set-version -p data_privacy_macros -p data_privacy --bump minor
```

After the version is bumped, an automated process should publish a new version within the next 48 hours.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft
trademarks or logos is subject to and must follow
[Microsoft's Trademark & Brand Guidelines](https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship.
Any use of third-party trademarks or logos are subject to those third-party's policies.
