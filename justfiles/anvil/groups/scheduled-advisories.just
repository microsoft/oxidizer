# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

[group("anvil")]
anvil-scheduled-advisories: anvil-scheduled-advisories-validate-prereqs \
    anvil-deny \
    anvil-audit \
    anvil-aprz \
    anvil-clippy

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
