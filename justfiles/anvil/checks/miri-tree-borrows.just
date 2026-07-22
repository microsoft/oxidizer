# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Stricter miri profiles. Each runs miri over the full workspace
# with a profile-specific MIRIFLAGS plus a profile-specific
# `--cfg miri_<profile>` set via RUSTFLAGS. Per-test opt-outs use
# `#[cfg_attr(miri_<profile>, ignore = "<reason>")]`, keeping the
# suppression next to the test rather than in a sidecar file.
# The cfg names are declared in the workspace check-cfg list
# (regions/cargo-lints-body.toml) so non-miri builds don't warn.
#
# These profiles always run full-workspace (scheduled-runtime-analysis
# does not set ANVIL_INCLUDE_AFFECTED); the --skip / affected guards
# below only apply if an adopter invokes them locally with the env
# vars set.
[script("pwsh")]
anvil-miri-tree-borrows: anvil-miri-tree-borrows-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    $env:MIRIFLAGS = "-Zmiri-tree-borrows $($env:MIRIFLAGS)".Trim()
    $env:RUSTFLAGS = "--cfg miri_tree_borrows $($env:RUSTFLAGS)".Trim()
    & cargo '+{{ rust_nightly }}' miri test --all-features --tests @pkg
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# The three nightly-miri profiles share the same toolchain prereqs
# as anvil-miri; they only differ in the MIRIFLAGS / RUSTFLAGS the
# recipe sets. Separate setup recipes keep the per-check fan-out
# regular (groups.just can iterate without special-casing).
[group("anvil-setup")]
anvil-miri-tree-borrows-setup installer="install": anvil-component-nightly-miri-install anvil-component-nightly-rust-src-install

[group("anvil-setup")]
anvil-miri-tree-borrows-validate-prereqs: anvil-component-nightly-miri-validate-prereqs anvil-component-nightly-rust-src-validate-prereqs
