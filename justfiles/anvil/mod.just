# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# Entry point for the anvil just recipe tree. The user's root Justfile
# imports this single file; everything else lives in sibling .just files
# pulled in here.

# All multi-statement recipes in this tree use [script("pwsh")]. pwsh
# is preinstalled on Windows (10+), GH/ADO hosted Linux + Windows
# runners, and is installable on macOS/Linux via Homebrew / upstream
# installer. We chose pwsh over bash because:
#
#   - just's shebang dispatch (#!/usr/bin/env bash) requires `cygpath`
#     on Windows, which is only on PATH inside Git Bash. Plain
#     PowerShell can't run shebang recipes.
#   - just's [script("bash")] attribute passes Windows tempfile paths
#     to bash unescaped, and bash interprets the backslashes as escape
#     characters — every recipe fails with a mangled path.
#   - [script("pwsh")] works from plain PowerShell with no PATH
#     augmentation and no path translation. pwsh handles Windows
#     paths natively.
#
# The existing `_anvil-require pwsh` recipe already established
# pwsh as a tools-floor requirement, so requiring it as the recipe
# interpreter is consistent.
#
# Single-command recipes (e.g. `cargo deny check`) are plain just
# recipes that inherit the adopter's default shell — they're
# shell-agnostic.
#
# IMPORTANT: [script(...)] requires `set unstable`. The adopter's
# root justfile must declare it (typically as a top-level line).
# anvil's mod.just does NOT redeclare it, to avoid conflicting
# with adopters who already have it.

import 'helpers.just'
import 'checks/aprz.just'
import 'checks/audit.just'
import 'checks/bench.just'
import 'checks/bolero.just'
import 'checks/careful.just'
import 'checks/cargo-hack.just'
import 'checks/cargo-sort.just'
import 'checks/clippy.just'
import 'checks/deny.just'
import 'checks/doc-build.just'
import 'checks/doc-test.just'
import 'checks/ensure-no-cyclic-deps.just'
import 'checks/ensure-no-default-features.just'
import 'checks/examples.just'
import 'checks/external-types.just'
import 'checks/fmt.just'
import 'checks/license-headers.just'
import 'checks/llvm-cov.just'
import 'checks/loom.just'
import 'checks/miri.just'
import 'checks/miri-race-coverage.just'
import 'checks/miri-strict-provenance.just'
import 'checks/miri-tree-borrows.just'
import 'checks/mutants-diff.just'
import 'checks/mutants-full.just'
import 'checks/pr-title.just'
import 'checks/readme-check.just'
import 'checks/semver-check.just'
import 'checks/spellcheck.just'
import 'checks/udeps.just'
import 'groups/pr-fast.just'
import 'groups/pr-slow.just'
import 'groups/pr-test.just'
import 'groups/pr-runtime-analysis.just'
import 'groups/pr-mutants.just'
import 'groups/scheduled-test.just'
import 'groups/scheduled-advisories.just'
import 'groups/scheduled-runtime-analysis.just'
import 'groups/scheduled-exhaustive.just'
import 'tiers.just'
import 'tools.just'
import 'versions.just'

# Friendly default: `just anvil` runs the PR tier.
alias anvil := anvil-pr
