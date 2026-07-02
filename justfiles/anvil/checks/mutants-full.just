# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Unscoped. Scheduled-exhaustive deliberately runs over the whole
# workspace regardless of diff.
[script("pwsh")]
anvil-mutants-full: anvil-mutants-full-validate-prereqs
    $ErrorActionPreference = 'Stop'
    & cargo mutants --workspace --no-shuffle --jobs 0
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# --- scheduled-exhaustive members (mutants-full reuses cargo-mutants;
#     cargo-hack and bench are dedicated tools) ---

[group("anvil-setup")]
anvil-mutants-full-setup installer="install": (anvil-tool-cargo-mutants-install installer)

[group("anvil-setup")]
anvil-mutants-full-validate-prereqs: anvil-tool-cargo-mutants-validate-prereqs
