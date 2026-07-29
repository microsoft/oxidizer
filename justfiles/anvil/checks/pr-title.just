# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Unscoped (PR title is not source-related).
#
# Validates that $env:PR_TITLE matches Conventional Commits when set;
# no-op when unset. Cloud workflows inject PR_TITLE explicitly: GH Actions
# from ${{ github.event.pull_request.title }}; ADO resolves it via the REST
# API in steps/group.yml (ADO has no PR-title predefined variable). Locally
# it skips silently when PR_TITLE is unset -- there is no reliable way to
# recover the PR title, so the recipe defers the check to cloud workflows.
# It does NOT tolerate a non-empty-but-wrong value: a misconfigured cloud
# variable (e.g. an unexpanded pipeline macro) is a real error and fails
# the check loudly rather than silently passing.

# Validate the pull request title.
[script("pwsh")]
anvil-pr-title: anvil-pr-title-validate-prereqs
    $title = $env:PR_TITLE
    if (-not $title) {
        Write-Host 'anvil-pr-title: PR_TITLE env var not set; skipping (check runs in cloud workflows)'
        exit 0
    }
    if ($title -notmatch '^(feat|fix|chore|docs|refactor|test|build|ci|perf|revert)(\([^)]+\))?!?: .+') {
        Write-Error "PR title '$title' does not match Conventional Commits"
        exit 1
    }

# pr-title is a pwsh script; no cargo tool to install.

# Prepare the PowerShell prerequisite for pull request title validation.
[group("anvil-setup")]
anvil-pr-title-setup installer="install": anvil-tool-pwsh-validate-prereqs

# Validate prerequisites for the `anvil-pr-title` recipe.
[group("anvil-setup")]
anvil-pr-title-validate-prereqs: anvil-tool-pwsh-validate-prereqs
