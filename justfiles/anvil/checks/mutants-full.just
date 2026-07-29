# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Unscoped. Scheduled-exhaustive deliberately runs over the whole
# workspace regardless of diff.

# Run exhaustive mutation testing across the workspace.
[script("pwsh")]
anvil-mutants-full: anvil-mutants-full-validate-prereqs
    $ErrorActionPreference = 'Stop'
    & cargo mutants --workspace --no-shuffle --jobs 0
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# --- scheduled-exhaustive members (mutants-full reuses cargo-mutants;
#     cargo-hack and bench are dedicated tools) ---

# Install prerequisites for the `anvil-mutants-full` recipe.
[group("anvil-setup")]
anvil-mutants-full-setup installer="install": (anvil-tool-cargo-mutants-install installer)

# Validate prerequisites for the `anvil-mutants-full` recipe.
[group("anvil-setup")]
anvil-mutants-full-validate-prereqs: anvil-tool-cargo-mutants-validate-prereqs
