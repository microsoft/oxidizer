# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Modified tier. cargo-fmt is a rustup component. We invoke it via the
# pinned nightly (see versions.just) because rustfmt.toml uses
# unstable_features = true (imports_granularity, group_imports,
# format_code_in_doc_comments). Floating nightly would mean
# format-drift breaking cloud workflows on rustup updates — the same trap we
# explicitly avoid for udeps/miri/careful/external-types.

# Check Rust source formatting.
[script("pwsh")]
anvil-fmt: anvil-fmt-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo '+{{ rust_nightly }}' fmt --all --check
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Per-check setup + validate-prereqs
#
# Each check has matching `*-setup` and `*-validate-prereqs` recipes
# that install / verify the tools and components it needs. The setup
# recipes accept an `installer=install|binstall` parameter that
# forwards to the underlying tool-install recipes; the validate-prereqs
# recipes take no parameters.
#
# These are the building blocks for `anvil-<group>-setup`
# (groups.just) and `anvil-<tier>-setup` (tiers.just) — each
# group/tier-level recipe is just a fan-out over the per-check
# setup/validate-prereqs of its members.

# --- pr-fast members ---

# Install prerequisites for the `anvil-fmt` recipe.
[group("anvil-setup")]
anvil-fmt-setup installer="install": anvil-component-nightly-rustfmt-install

# Validate prerequisites for the `anvil-fmt` recipe.
[group("anvil-setup")]
anvil-fmt-validate-prereqs: anvil-component-nightly-rustfmt-validate-prereqs
