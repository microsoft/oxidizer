# Releasing Oxidizer Packages

Oxidizer prepares versions on demand with Folo's
[`increment-versions` skill](../.github/skills/increment-versions/SKILL.md) in
[standalone mode](https://folo-rs.github.io/folo/cargo-release-plan/advanced/standalone.html).
The resulting PR contains the version, dependency, lockfile, and changelog
changes. Merging it leaves publication to the existing Microsoft automation.

The vendored skill comes from `folo-rs/folo` commit
`fcc52639a902c8d8ad8000a58e0dfc39637bb6ac` and uses
`cargo-release-plan` 0.8.4. Update the complete skill directory, tool version,
and schema details together when adopting a newer revision. Its files match the
upstream directory except for the YAML frontmatter added to `SKILL.md` so
Copilot can register it as a repository skill.

## Prerequisites

```powershell
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
cargo install cargo-release-plan --version "=0.8.4" --locked
cargo release-plan version
just anvil-tool-cargo-semver-checks-install
cargo semver-checks --version
```

The installed tool must report plan/report/prepared schema 8, decision schema 2,
compatibility schema 3, and release-context schema 2. Keep generated evidence
outside the repository or in an ignored directory.

## Prepare a release PR

1. Start a feature branch from the intended merge target.
2. Ask Copilot to run the skill, naming the release-history ref and any stacked
   parent explicitly:

   ```text
   Run /increment-versions to prepare a release PR for this workspace.
   Refresh origin/main and use it as release history. There is no unmerged parent.
   Do not merge or publish.
   ```

3. Let the skill assess the complete change, resolve dependent-package effects,
   apply the plan, and produce fresh compatibility evidence.
4. In the same PR, prepend an entry to every package `CHANGELOG.md` in the final
   plan. Use the planned version and release date, summarize the complete
   consumer-visible change since the package anchor, include dependency-only
   movements, and preserve historical entries.
5. Rerun the skill's preparation and final verification after the changelog
   edits so its evidence covers the completed PR.
6. Review the complete **Version/release plan**, changelogs, compatibility
   results, and normal repository checks before merging.

For a stacked release PR, identify its actual parent target in the request
instead of saying there is no unmerged parent. Reassess if code, release history,
or the target changes during review.

## Boundaries

The skill does not merge, publish, tag, or grant permission for those actions.
Standalone mode does not add publication configuration, Trusted Publisher
setup, registry gates, or replacement workflows. Merging the reviewed version PR
continues to trigger the existing publication automation.
