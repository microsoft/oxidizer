# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Scheduled groups

[group("anvil")]
anvil-scheduled-test: anvil-scheduled-test-validate-prereqs \
    anvil-llvm-cov \
    anvil-doc-test \
    anvil-examples

[group("anvil-setup")]
anvil-scheduled-test-setup installer="install": \
    (anvil-llvm-cov-setup installer) \
    (anvil-doc-test-setup installer) \
    (anvil-examples-setup installer)

[group("anvil-setup")]
anvil-scheduled-test-validate-prereqs: \
    anvil-llvm-cov-validate-prereqs \
    anvil-doc-test-validate-prereqs \
    anvil-examples-validate-prereqs
