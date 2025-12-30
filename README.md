<div align="center">
 <img src="./logo.svg" alt="Oxidizer Logo" width="96">

# The Oxidizer Project

[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

</div>

This repository contains a set of crates that help you build robust highly scalable services in Rust.

- [Crates](#crates)
- [About this Repo](#about-this-repo)
    - [Adding New Crates](#adding-new-crates)
    - [Publishing Crates](#publishing-crates)
    - [Documenting Crates](#documenting-crates)
    - [CI Workflows](#ci-workflows)
    - [Pull Request Gates](#pull-request-gates)
- [Trademarks](#trademarks)

## Crates

These are the crates built out of this repo:

- [`bytesbuf`](./crates/bytesbuf/README.md) - Manipulate sequences of bytes for efficient I/O.
- [`data_privacy`](./crates/data_privacy/README.md) - Mechanisms to classify, manipulate, and redact sensitive data.
- [`data_privacy_macros`](./crates/data_privacy_macros/README.md) - Macros for the `data_privacy` crate.
- [`data_privacy_macros_impl`](./crates/data_privacy_macros_impl/README.md) - Macros for the `data_privacy` crate.
- [`fundle`](./crates/fundle/README.md) - Compile-time safe dependency injection for Rust.
- [`fundle_macros`](crates/fundle_macros/README.md) - Macros for the `fundle` crate.
- [`fundle_macros_impl`](crates/fundle_macros_impl/README.md) - Macros for the `fundle` crate.
- [`ohno`](./crates/ohno/README.md) - High-quality Rust error handling.
- [`ohno_macros`](./crates/ohno_macros/README.md) - Macros for the `ohno` crate.
- [`recoverable`](./crates/recoverable/README.md) - Recovery information and classification for resilience patterns.
- [`thread_aware`](./crates/thread_aware/README.md) - Facilities to support thread-isolated state.
- [`thread_aware_macros`](./crates/thread_aware_macros/README.md) - Macros for the `thread_aware` crate.
- [`thread_aware_macros_impl`](./crates/thread_aware_macros_impl/README.md) - Macros for the `thread_aware` crate.
- [`tick`](./crates/tick/README.md) - Provides primitives to interact with and manipulate machine time.

## About this Repo

The following sections explain the overall engineering process we use
in this repo.

### Adding New Crates

Adding a new crate to this repo is done by running the `scripts\add-crate.ps1` script.
It will prompt you for a few bits of state, and then will get everything wired up that
needs to be.

The `add-crate` script does the following:

- Adds an entry for the crate to the [Crates](#crates) section in this README file.

- Adds an entry for the crate to the top-level [CHANGELOG.md](./CHANGELOG.md) file.

- Prepares a `README.md` file for the crate, setup for use with [
  `cargo-doc2readme`](https://crates.io/crates/cargo-doc2readme)
  with a set of appropriate CI badges.

- Creates an empty `CHANGELOG.md` file for the crate, which will later get populated by the `scripts\release-crate.ps1`
  script.

- Creates placeholder `logo.png` and `favicon.ico` files for the crate, which you're expected to replace with legit
  crab-themed
  logo and icon.

### Publishing Crates

Releasing new versions of crates to [crates.io](https://crates.io) is handled by
an internal Microsoft automation process. To release a new version of any crate, follow
this simple process:

1. Make sure the changes you want to release have all been committed to the repo.

2. Create a branch off of main.

3. Run `./scripts/release-crate.ps1 <crate_name> [new_version]` to bump a crate's version and update the crate's
   `CHANGELOG.md` file.
   Run the script many times if you want to release several crates in the same PR.

4. Create a PR like normal to push changes out.

Once your PR is merged, automation will kick in. It will tag the
commit and push the crate to crates.io.

### Documenting Crates

We want our crates to have world-class documentation such that our customers can enjoy discovering and using our
features. We expect our Rust code to be fully documented in the normal Rust way, and we introduce two doc-related
automation processes:

- The `README.md` file in each crate's directory is auto-generated from the crate-level documentation.
  We use the [`cargo-doc2readme`](https://crates.io/crates/cargo-doc2readme) tool which reads the crate docs, resolves intra-doc links, and
  generates the `README.md` file using a shared template. A pull request gate ensures the `README.md` file
  always reflects the latest crate documentation.

- The `CHANGELOG.md` file in each crate's directory is auto-generated from the commits to a crate's directory by the
  `scripts/release-crate.ps1` script.

To generate documentation locally with all features enabled (including feature-gated items), run:

```shell
.\scripts\generate-docs.ps1
```

This requires the Rust nightly toolchain to be installed. The script will generate documentation
and open it in your default browser.

### CI Workflows

We have two primary workflows:

- `main`. Runs on all pull requests and commits to the main branch. This
  performs quite a bit of validation to ensure high-quality outcomes. Any issues
  found by this workflow blocks the pull request from being merged.

- `nightly`. Runs nightly on the main branch. This executes repo-wide mutation testing
  (as opposed to the main workflow which does incremental testing). Any issues
  found by this workflow result in an issue being opened reporting the problem.

### Pull Request Gates

We strive to deliver high-quality code and as such, we've put in place a number of PR gates, described here:

- **Build**. We build all the crates in the repo for Windows and Linux.
  We use [`cargo-hack`](https://crates.io/crates/cargo-hack) to iterate through
  different crate feature combinations to make sure everything builds properly.

- **Testing**. We run `cargo nextest --all-features` to run every normal test and documentation test in the repo.

- **Code Coverage**. We calculate code coverage for the whole repo using [
  `cargo-llvm-cov`](https://crates.io/crates/cargo-llvm-cov).
  We capture coverage for Windows and Linux, with `--all-features` and `--no-features`. Coverage is collected using
  the nightly Rust compiler which makes it possible to use `coverage(off)` annotations in the source code to suppress
  coverage collection for a chunk of code. We require 100% coverage for any checked in code.

- **Mutation Testing**. We use [`cargo-mutants`](https://crates.io/crates/cargo-mutants) to help maintain
  high test quality.

- **Source Linting**. We run Clippy with most warnings enabled and all treated as errors.

- **Doc Linting**. We lint documentation to help find bad links and other anti-patterns.

- **Source Formatting**. We ensure the source code complies with the Rust standard format.

- **Cargo.toml Formatting**. We use [`cargo-sort`](https://crates.io/crates/cargo-sort) to keep Cargo.toml
  files in a consistent format and layout.

- **Unsafe Verification**. We use Miri and [`cargo-careful`](https://crates.io/crates/cargo-careful) to verify that our
  unsafe code doesn't induce undefined behaviors.

- **External Type Exposure**. We use [`cargo-external-types`](https://crates.io/crates/cargo-external-types) to track
  which external types our crates depend on. Exposing a 3P type from a crate creates a coupling between the crate and
  the exporter
  of the type which can be problematic over time. This check is there to prevent unintentional exposure. If the exposure
  is intentional,
  it's a simple matter of adding an exclusion for it to the crate's `Cargo.toml` file.

- **Default Features**. We use [
  `cargo-ensure-no-default-features`](https://crates.io/crates/cargo-ensure-no-default-features) to make
  sure the dependencies pulled in by the top-level Cargo.toml are all annotated with `default-features = false`.
  Individual crates that use
  these dependencies are then responsible for stating exactly which features they need. This is designed to minimize
  build times for
  our customers.

- **Cyclic Dependencies**. We use [`cargo-ensure-no-cyclic-deps`](https://crates.io/crates/cargo-ensure-no-cyclic-deps)
  to ensure the
  crates in the repo don't create funny referential cycles using `dev-dependencies`. Things break or get difficult when
  these cycles exist.

- **Unneeded Dependencies**. We use [`cargo-udeps`](https://crates.io/crates/cargo-udeps) to ensure our crates don't
  have superfluous
  dependencies.

- **Dependency Validation**. We use [`cargo-deny`](https://crates.io/crates/cargo-deny) to ensure our dependencies
  have acceptable licenses and don't contain known vulnerabilities.

- **Semantic Version Compatibility**. We use [`cargo-semver-checks`](https://crates.io/crates/cargo-semver-checks) to ensure
  our API surface maintains the compatibility guarantees implies by semantic versioning.

- **PR Title**. Every PR submitted to this repo must follow
  the [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/)
  specification. We use these PR titles as part of our automatic change log generation logic.

- **License Headers**. We ensure all source files have the requisite license header. The headers are described in
  the `.github\license-check` directory.

- **Spell Checking**. We use [cargo-spellcheck](https://crates.io/crates/cargo-spellcheck) to help our docs have fewer typos.

- **README Content**. We use [`cargo-doc2readme`](https://crates.io/crates/cargo-doc2readme) to ensure each crate's `README.md`
  file matches the crate's current crate-level documentation.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft
trademarks or logos is subject to and must follow
[Microsoft's Trademark & Brand Guidelines](https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft
sponsorship.
Any use of third-party trademarks or logos are subject to those third-party's policies.
