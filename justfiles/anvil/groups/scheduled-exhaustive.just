# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

[group("anvil")]
anvil-scheduled-exhaustive: anvil-scheduled-exhaustive-validate-prereqs \
    anvil-mutants-full \
    anvil-cargo-hack \
    anvil-bench

[group("anvil-setup")]
anvil-scheduled-exhaustive-setup installer="install": \
    (anvil-mutants-full-setup installer) \
    (anvil-cargo-hack-setup installer) \
    (anvil-bench-setup installer)

[group("anvil-setup")]
anvil-scheduled-exhaustive-validate-prereqs: \
    anvil-mutants-full-validate-prereqs \
    anvil-cargo-hack-validate-prereqs \
    anvil-bench-validate-prereqs
