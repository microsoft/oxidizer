# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Run the scheduled advisory checks.
[group("anvil")]
anvil-scheduled-advisories: anvil-scheduled-advisories-validate-prereqs \
    anvil-deny \
    anvil-audit \
    anvil-aprz \
    anvil-clippy

# Install prerequisites for the `anvil-scheduled-advisories` recipe.
[group("anvil-setup")]
anvil-scheduled-advisories-setup installer="install": \
    (anvil-deny-setup installer) \
    (anvil-audit-setup installer) \
    (anvil-aprz-setup installer) \
    (anvil-clippy-setup installer)

# Validate prerequisites for the `anvil-scheduled-advisories` recipe.
[group("anvil-setup")]
anvil-scheduled-advisories-validate-prereqs: \
    anvil-deny-validate-prereqs \
    anvil-audit-validate-prereqs \
    anvil-aprz-validate-prereqs \
    anvil-clippy-validate-prereqs
