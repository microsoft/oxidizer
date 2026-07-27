# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

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

# Run the slow pull request checks.
[group("anvil")]
anvil-pr-slow: anvil-pr-slow-validate-prereqs anvil-pr-test anvil-pr-runtime-analysis anvil-pr-mutants

# Install prerequisites for the `anvil-pr-slow` recipe.
[group("anvil-setup")]
anvil-pr-slow-setup installer="install": \
    (anvil-pr-test-setup installer) \
    (anvil-pr-runtime-analysis-setup installer) \
    (anvil-pr-mutants-setup installer)

# Validate prerequisites for the `anvil-pr-slow` recipe.
[group("anvil-setup")]
anvil-pr-slow-validate-prereqs: \
    anvil-pr-test-validate-prereqs \
    anvil-pr-runtime-analysis-validate-prereqs \
    anvil-pr-mutants-validate-prereqs
