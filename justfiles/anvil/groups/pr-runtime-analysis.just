# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

[group("anvil")]
anvil-pr-runtime-analysis: anvil-pr-runtime-analysis-validate-prereqs \
    anvil-miri \
    anvil-careful \
    anvil-loom \
    anvil-bolero

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
