# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Run the pull request runtime analysis.
[group("anvil")]
anvil-pr-runtime-analysis: anvil-pr-runtime-analysis-validate-prereqs \
    anvil-miri \
    anvil-careful \
    anvil-loom \
    anvil-bolero

# Install prerequisites for the `anvil-pr-runtime-analysis` recipe.
[group("anvil-setup")]
anvil-pr-runtime-analysis-setup installer="install": \
    (anvil-miri-setup installer) \
    (anvil-careful-setup installer) \
    (anvil-loom-setup installer) \
    (anvil-bolero-setup installer)

# Validate prerequisites for the `anvil-pr-runtime-analysis` recipe.
[group("anvil-setup")]
anvil-pr-runtime-analysis-validate-prereqs: \
    anvil-miri-validate-prereqs \
    anvil-careful-validate-prereqs \
    anvil-loom-validate-prereqs \
    anvil-bolero-validate-prereqs
