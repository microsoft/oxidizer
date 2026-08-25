# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Modified tier.
#
# `--check-format` makes the formatting diff fail the check, not just the
# sort order. cargo-sort 2.1.2+ downgraded formatting-only diffs to
# warnings by default (PR #129); without this flag a local 2.1.2+ install
# would pass on formatting drift that the pinned version still rejects.
# The flag exists in 2.0.2 too (where it is a no-op, since that version
# already errors on formatting), so it is safe across the supported range.
#
# `--grouped` keeps the blank lines between groups of key/value pairs
# (e.g. blank-line-separated dependency blocks) intact, matching
# oxidizer-github's convention; without it cargo-sort would flag those
# blank lines as a formatting diff and fail the check.

# Check that Cargo.toml dependency tables are sorted.
[script("pwsh")]
anvil-cargo-sort: anvil-cargo-sort-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo sort --workspace --grouped --check --check-format
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Install prerequisites for the `anvil-cargo-sort` recipe.
[group("anvil-setup")]
anvil-cargo-sort-setup installer="install": (anvil-tool-cargo-sort-install installer)

# Validate prerequisites for the `anvil-cargo-sort` recipe.
[group("anvil-setup")]
anvil-cargo-sort-validate-prereqs: anvil-tool-cargo-sort-validate-prereqs
