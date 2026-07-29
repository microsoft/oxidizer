# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Run the scheduled exhaustive checks.
[group("anvil")]
anvil-scheduled-exhaustive: anvil-scheduled-exhaustive-validate-prereqs \
    anvil-mutants-full \
    anvil-cargo-hack \
    anvil-bench

# Install prerequisites for the `anvil-scheduled-exhaustive` recipe.
[group("anvil-setup")]
anvil-scheduled-exhaustive-setup installer="install": \
    (anvil-mutants-full-setup installer) \
    (anvil-cargo-hack-setup installer) \
    (anvil-bench-setup installer)

# Validate prerequisites for the `anvil-scheduled-exhaustive` recipe.
[group("anvil-setup")]
anvil-scheduled-exhaustive-validate-prereqs: \
    anvil-mutants-full-validate-prereqs \
    anvil-cargo-hack-validate-prereqs \
    anvil-bench-validate-prereqs
