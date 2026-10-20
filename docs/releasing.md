# Releasing Oxidizer Packages

Oxidizer uses Folo's documented
[standalone version-increment flow](https://folo-rs.github.io/folo/cargo-release-plan/advanced/standalone.html).
It prepares a reviewed PR with version bumps and changelog entries while leaving
publication to the existing Microsoft automation.

The complete upstream [`increment-versions` skill](../.github/skills/increment-versions/SKILL.md)
is copied from `folo-rs/folo` commit
`fcc52639a902c8d8ad8000a58e0dfc39637bb6ac`. The selected published CLI is
`cargo-release-plan` 0.8.4, whose reported plan/report/prepared schemas are 8,
decision schema is 2, compatibility schema is 3, and release-context schema is
2. Update the complete skill directory and this adoption record together when
the schemas change.

The published `cargo-release-plan-action` v0.1.0 selects CLI 0.5.10 and is not
schema-compatible with this skill revision. Do not install that action, a
publication configuration, or replacement publishing workflows for the
standalone flow.

## Install and inspect

Use PowerShell 7.6 or newer and install the selected published release with its
lockfile:

```powershell
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
cargo install cargo-release-plan --version "=0.8.4" --locked
cargo release-plan version
cargo release-plan --help
just anvil-tool-cargo-semver-checks-install
cargo semver-checks --version
```

Before editing release inputs, run the Folo inspection commands from the
workspace root:

```powershell
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
git status --short
cargo metadata --no-deps --format-version 1
cargo release-plan version
```

Keep generated reports, plans, and compatibility evidence outside the
repository or in an already ignored directory. Do not commit generated plans as
an alternative source of version truth.

## Prepare a release PR

1. Open a Copilot session on a new feature branch from updated `main`.
2. Ask Copilot:

   ```text
   Run /increment-versions to prepare a release PR for this workspace.
   Refresh origin/main and use it as release history. There is no unmerged parent.
   Do not merge or publish.
   ```

3. Let the agent check prerequisites, assess changes, update versions, and
   prepare the PR. It will ask for missing inputs or installation permission.
4. After applying the resolved plan, prepend an entry to each affected package's
   `CHANGELOG.md`. Use the planned version and current date; summarize all
   consumer-visible changes since the package anchor, including dependency-only
   movements. Do not rewrite historical entries.
5. Because the checkout changed after the plan was applied, rerun the skill's
   preparation, compatibility, and final verification steps after adding them.
6. Review the PR's **Version/release plan**, changelog entries, dependent-package
   changes, and unresolved compatibility checks. Require the normal repository
   checks before approving.
7. Merge under the normal review policy, then monitor the existing internal
   Microsoft publication automation and verify the expected versions on crates.io.

For a stacked release PR, identify its actual parent target in the request
instead of saying there is no unmerged parent. If code or history changes during
review, ask the agent to reassess before merging.

If Copilot cannot find the skill, start a new session or use `/skills reload`
in Copilot CLI.

## Boundaries

Merging a version PR still triggers the existing publication automation; the
skill itself does not publish or grant merge permission. No additional local
publish command is needed. Standalone mode does not add `.cargo/release_plan.toml`,
Trusted Publisher setup, registry gates, or GitHub release workflows.
