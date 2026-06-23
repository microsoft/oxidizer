# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
#
# Pinned versions used by the anvil check tree.
#
# Everything anvil installs (cargo subcommands + rustup toolchains)
# is pinned here. The pinning policy:
#   - On install (`-install` recipes): exactly this version (`=<pin>` for
#     cargo subcommands, exact ref for rustup toolchains). Pulling
#     "latest-matching" at install time is a cloud-workflow reproducibility risk --
#     an upstream release between yesterday's green build and today's
#     PR can break things (cargo-spellcheck 0.15.7's em-dash regression
#     is the canonical case). The `=` constraint locks the install to
#     the version the catalog was validated against.
#   - On validate-prereqs (`-validate-prereqs` recipes): the installed
#     version must be `>= <pin>`. A user who has manually upgraded a
#     tool for their own reasons (e.g. needing an unreleased bugfix)
#     is not downgraded by setup. The validate gate uses
#     `installed >= pin`, so newer is fine.
#
# To bump a pin, edit the version in place and re-run
# `cargo anvil`. The dirty-file flow preserves the edit on
# subsequent runs. To add a new tool, append a variable and the matching
# `-install`/`-validate-prereqs` pair in `tools.just`. To remove a tool,
# delete its variable and recipes (and any check that depends on them).

# ============================================================================
# Rustup toolchains
# ============================================================================

# General nightly used by udeps, miri, careful, and any future
# nightly-dependent check. Bumped on a regular cadence (monthly is a
# reasonable default) when an adopter has time to absorb formatting /
# lint / API-surface drift. Pin only -- do not use bare `nightly` here.
rust_nightly := "nightly-2026-02-10"

# Pinned narrowly to the rustdoc JSON schema version that the currently
# selected cargo-check-external-types release accepts. cargo-check-
# external-types embeds a specific rustdoc-types crate version; if the
# nightly's emitted JSON format_version drifts past it, every run fails
# with "produces JSON format version X, but this tool requires
# format version Y" -- which is a tooling-incompat, not a real API
# violation. Bump this alongside any cargo-check-external-types upgrade.
rust_nightly_external_types := "nightly-2025-10-18"

# ============================================================================
# Cargo subcommands
# ============================================================================

cargo_aprz_version := "1.0.0"
cargo_audit_version := "0.22.2"
cargo_bolero_version := "0.13.4"
cargo_careful_version := "0.4.9"
cargo_check_external_types_version := "0.4.0"
cargo_coverage_gate_version := "0.1.0"
cargo_delta_version := "0.3.1"
cargo_deny_version := "0.19.0"
cargo_doc2readme_version := "0.6.4"
cargo_ensure_no_cyclic_deps_version := "0.2.0"
cargo_ensure_no_default_features_version := "1.0.0"
cargo_hack_version := "0.6.41"
cargo_heather_version := "0.2.1"
cargo_llvm_cov_version := "0.8.4"
cargo_mutants_version := "26.1.2"
cargo_nextest_version := "0.9.122"
cargo_semver_checks_version := "0.46.0"
cargo_sort_version := "2.0.2"
cargo_spellcheck_version := "0.15.7"
cargo_udeps_version := "0.1.60"
