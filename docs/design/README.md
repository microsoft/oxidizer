# Repository verification

This repository uses Cargo Anvil as the source of truth for Rust verification.
Use the generated Anvil recipes locally and the generated Anvil workflows in
CI. Do not maintain parallel implementations of Anvil checks.

Keep repository-specific automation only for capabilities outside Anvil's
scope, such as release tooling and tests for that tooling. Generated Anvil
files are updated with `cargo anvil`, not edited directly.

## Crate designs

- [`fakeable`](fakeable.md) describes the generated real/fake wrapper model and
  its optional Mockall integration.
