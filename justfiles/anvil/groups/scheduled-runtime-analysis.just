# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# scheduled-runtime-analysis is the full-workspace counterpart of
# pr-runtime-analysis: it re-runs miri unscoped (PR-tier miri is
# impact-scoped, so crates untouched by a PR never get miri coverage)
# and adds the three stricter miri profiles (tree-borrows, strict-
# provenance, race-coverage) which are too expensive for PR.
[group("anvil")]
anvil-scheduled-runtime-analysis: anvil-scheduled-runtime-analysis-validate-prereqs \
    anvil-miri \
    anvil-miri-tree-borrows \
    anvil-miri-strict-provenance \
    anvil-miri-race-coverage

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
