# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

[group("anvil")]
anvil-pr-fast: anvil-pr-fast-validate-prereqs \
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

# Group-level setup + validate-prereqs
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
