# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../docs/design/checks.md for the catalog and rationale.

# Each group is one cloud-workflow job. Within a group, checks run sequentially.

# PR groups
# ===========================================================================

[group("anvil")]
anvil-pr-fast: \
    anvil-fmt \
    anvil-clippy \
    anvil-cargo-sort \
    anvil-license-headers \
    anvil-ensure-no-cyclic-deps \
    anvil-ensure-no-default-features \
    anvil-doc-build \
    anvil-readme-check \
    anvil-spellcheck \
    anvil-pr-title \
    anvil-deny \
    anvil-audit \
    anvil-udeps \
    anvil-semver-check \
    anvil-external-types \
    anvil-aprz

# pr-slow is the single PR-tier group for everything that takes more
# than ~30s per crate (tests, stricter runtimes, mutation testing).
# It's internally split into three sub-recipes so individual concerns
# can be invoked locally without dragging the others along:
#
#   slow1: tests + coverage (replaces the former pr-test group)
#   slow2: stricter-runtime correctness (miri, careful)
#   slow3: mutation testing
#
# Cloud workflows run pr-slow as ONE job per OS leg -- the sub-recipes run
# sequentially within. This trades per-leg wall-clock for fewer
# orchestration jobs and a flatter PR check graph. Individual sub-
# recipes are runnable on their own locally:
#
#   $ just anvil-pr-test   # tests + coverage only
#   $ just anvil-pr-runtime-analysis   # miri + careful only
#   $ just anvil-pr-mutants   # mutants only
[group("anvil")]
anvil-pr-slow: anvil-pr-test anvil-pr-runtime-analysis anvil-pr-mutants

[group("anvil")]
anvil-pr-test: \
    anvil-llvm-cov \
    anvil-doc-test \
    anvil-examples

[group("anvil")]
anvil-pr-runtime-analysis: \
    anvil-miri \
    anvil-careful \
    anvil-loom \
    anvil-bolero

[group("anvil")]
anvil-pr-mutants: anvil-mutants-diff

# Scheduled groups
# ===========================================================================

[group("anvil")]
anvil-scheduled-test: \
    anvil-llvm-cov \
    anvil-doc-test \
    anvil-examples

[group("anvil")]
anvil-scheduled-advisories: \
    anvil-deny \
    anvil-audit \
    anvil-aprz \
    anvil-clippy

# scheduled-runtime-analysis is the full-workspace counterpart of
# pr-runtime-analysis: it re-runs miri unscoped (PR-tier miri is
# impact-scoped, so crates untouched by a PR never get miri coverage)
# and adds the three stricter miri profiles (tree-borrows, strict-
# provenance, race-coverage) which are too expensive for PR.
[group("anvil")]
anvil-scheduled-runtime-analysis: \
    anvil-miri \
    anvil-miri-tree-borrows \
    anvil-miri-strict-provenance \
    anvil-miri-race-coverage

[group("anvil")]
anvil-scheduled-exhaustive: \
    anvil-mutants-full \
    anvil-cargo-hack \
    anvil-bench
# Group-level setup + validate-prereqs
# ===========================================================================
#
# Per-group recipes that fan out to the per-check setup/validate-prereqs
# from checks.just. Setup recipes accept `installer="install"|"binstall"`;
# validate-prereqs recipes take no parameters.

[group("anvil-setup")]
anvil-pr-fast-setup installer="install": \
    (anvil-fmt-setup installer) \
    (anvil-clippy-setup installer) \
    (anvil-cargo-sort-setup installer) \
    (anvil-license-headers-setup installer) \
    (anvil-ensure-no-cyclic-deps-setup installer) \
    (anvil-ensure-no-default-features-setup installer) \
    (anvil-doc-build-setup installer) \
    (anvil-readme-check-setup installer) \
    (anvil-spellcheck-setup installer) \
    (anvil-pr-title-setup installer) \
    (anvil-deny-setup installer) \
    (anvil-audit-setup installer) \
    (anvil-udeps-setup installer) \
    (anvil-semver-check-setup installer) \
    (anvil-external-types-setup installer) \
    (anvil-aprz-setup installer)

[group("anvil-setup")]
anvil-pr-fast-validate-prereqs: \
    anvil-fmt-validate-prereqs \
    anvil-clippy-validate-prereqs \
    anvil-cargo-sort-validate-prereqs \
    anvil-license-headers-validate-prereqs \
    anvil-ensure-no-cyclic-deps-validate-prereqs \
    anvil-ensure-no-default-features-validate-prereqs \
    anvil-doc-build-validate-prereqs \
    anvil-readme-check-validate-prereqs \
    anvil-spellcheck-validate-prereqs \
    anvil-pr-title-validate-prereqs \
    anvil-deny-validate-prereqs \
    anvil-audit-validate-prereqs \
    anvil-udeps-validate-prereqs \
    anvil-semver-check-validate-prereqs \
    anvil-external-types-validate-prereqs \
    anvil-aprz-validate-prereqs

[group("anvil-setup")]
anvil-pr-slow-setup installer="install": \
    (anvil-pr-test-setup installer) \
    (anvil-pr-runtime-analysis-setup installer) \
    (anvil-pr-mutants-setup installer)

[group("anvil-setup")]
anvil-pr-slow-validate-prereqs: \
    anvil-pr-test-validate-prereqs \
    anvil-pr-runtime-analysis-validate-prereqs \
    anvil-pr-mutants-validate-prereqs

[group("anvil-setup")]
anvil-pr-test-setup installer="install": \
    (anvil-llvm-cov-setup installer) \
    (anvil-doc-test-setup installer) \
    (anvil-examples-setup installer)

[group("anvil-setup")]
anvil-pr-test-validate-prereqs: \
    anvil-llvm-cov-validate-prereqs \
    anvil-doc-test-validate-prereqs \
    anvil-examples-validate-prereqs

[group("anvil-setup")]
anvil-pr-runtime-analysis-setup installer="install": \
    (anvil-miri-setup installer) \
    (anvil-careful-setup installer) \
    (anvil-loom-setup installer) \
    (anvil-bolero-setup installer)

[group("anvil-setup")]
anvil-pr-runtime-analysis-validate-prereqs: \
    anvil-miri-validate-prereqs \
    anvil-careful-validate-prereqs \
    anvil-loom-validate-prereqs \
    anvil-bolero-validate-prereqs

[group("anvil-setup")]
anvil-pr-mutants-setup installer="install": (anvil-mutants-diff-setup installer)

[group("anvil-setup")]
anvil-pr-mutants-validate-prereqs: anvil-mutants-diff-validate-prereqs

[group("anvil-setup")]
anvil-scheduled-test-setup installer="install": \
    (anvil-llvm-cov-setup installer) \
    (anvil-doc-test-setup installer) \
    (anvil-examples-setup installer)

[group("anvil-setup")]
anvil-scheduled-test-validate-prereqs: \
    anvil-llvm-cov-validate-prereqs \
    anvil-doc-test-validate-prereqs \
    anvil-examples-validate-prereqs

[group("anvil-setup")]
anvil-scheduled-advisories-setup installer="install": \
    (anvil-deny-setup installer) \
    (anvil-audit-setup installer) \
    (anvil-aprz-setup installer) \
    (anvil-clippy-setup installer)

[group("anvil-setup")]
anvil-scheduled-advisories-validate-prereqs: \
    anvil-deny-validate-prereqs \
    anvil-audit-validate-prereqs \
    anvil-aprz-validate-prereqs \
    anvil-clippy-validate-prereqs

[group("anvil-setup")]
anvil-scheduled-runtime-analysis-setup installer="install": \
    (anvil-miri-setup installer) \
    (anvil-miri-tree-borrows-setup installer) \
    (anvil-miri-strict-provenance-setup installer) \
    (anvil-miri-race-coverage-setup installer)

[group("anvil-setup")]
anvil-scheduled-runtime-analysis-validate-prereqs: \
    anvil-miri-validate-prereqs \
    anvil-miri-tree-borrows-validate-prereqs \
    anvil-miri-strict-provenance-validate-prereqs \
    anvil-miri-race-coverage-validate-prereqs

[group("anvil-setup")]
anvil-scheduled-exhaustive-setup installer="install": \
    (anvil-mutants-full-setup installer) \
    (anvil-cargo-hack-setup installer) \
    (anvil-bench-setup installer)

[group("anvil-setup")]
anvil-scheduled-exhaustive-validate-prereqs: \
    anvil-mutants-full-validate-prereqs \
    anvil-cargo-hack-validate-prereqs \
    anvil-bench-validate-prereqs