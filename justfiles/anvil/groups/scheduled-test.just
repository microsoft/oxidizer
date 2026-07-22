# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Scheduled groups

# Run the scheduled tests.
[group("anvil")]
anvil-scheduled-test: anvil-scheduled-test-validate-prereqs \
    anvil-llvm-cov \
    anvil-doc-test \
    anvil-examples

# Install prerequisites for the `anvil-scheduled-test` recipe.
[group("anvil-setup")]
anvil-scheduled-test-setup installer="install": \
    (anvil-llvm-cov-setup installer) \
    (anvil-doc-test-setup installer) \
    (anvil-examples-setup installer)

# Validate prerequisites for the `anvil-scheduled-test` recipe.
[group("anvil-setup")]
anvil-scheduled-test-validate-prereqs: \
    anvil-llvm-cov-validate-prereqs \
    anvil-doc-test-validate-prereqs \
    anvil-examples-validate-prereqs
