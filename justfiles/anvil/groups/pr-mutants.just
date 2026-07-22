# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

[group("anvil")]
anvil-pr-mutants: anvil-pr-mutants-validate-prereqs anvil-mutants-diff

[group("anvil-setup")]
anvil-pr-mutants-setup installer="install": (anvil-mutants-diff-setup installer)

[group("anvil-setup")]
anvil-pr-mutants-validate-prereqs: anvil-mutants-diff-validate-prereqs
