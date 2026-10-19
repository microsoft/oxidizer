---
name: increment-versions
description: Prepare an on-demand version-increment PR using cargo-release-plan. Use only when explicitly asked to prepare package versions or a release plan; Oxidizer uses standalone mode and keeps publication separate.
license: MIT
---

# On-demand version preparation

Use the upstream `increment-versions` skill in **standalone mode**, only when
explicitly asked to prepare package versions. Do not run it for ordinary code
PRs. Read [repository release policy](../../../docs/releasing.md) and follow
[AGENTS.md](../../../AGENTS.md).

## Prerequisite gate

Run this read-only preflight first, before loading upstream resources, asking for
planning inputs, fetching Git refs, or changing release inputs:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$missing = @()
if ($PSVersionTable.PSVersion -lt [version]'7.6') {
    $missing += 'PowerShell 7.6 or newer'
}
foreach ($command in @('git', 'cargo', 'rustc', 'cargo-release-plan', 'cargo-semver-checks')) {
    if (-not (Get-Command $command -CommandType Application -ErrorAction SilentlyContinue)) {
        $missing += $command
    }
}
if ($missing.Count -gt 0) {
    throw "Release preparation blocked. Missing prerequisites: $($missing -join ', '). See the installation commands in this skill."
}
git --version
cargo --version
rustc --version
cargo release-plan version
cargo semver-checks --version
```

Missing prerequisites or failed version probes **reject this invocation**.
Report the missing or broken tools and the relevant installation commands below,
then stop. Do not install automatically, continue planning, or treat a failed
probe as a compatibility pass.

### Installation commands

For `cargo-release-plan`, install or upgrade to the latest published stable
release, using that release's dependency lockfile:

```powershell
cargo install --locked cargo-release-plan
```

For `cargo-semver-checks`, use Anvil's installer so its version remains selected
by the repository catalog rather than a second tool-version list:

```powershell
just anvil-tool-cargo-semver-checks-install
```

If Git, PowerShell, or Cargo/Rust itself is missing, follow
[development environment setup](../../../DEVELOPMENT.md). Source installation
requires the tool's supported compiler and native build prerequisites; the
upstream installation guide below also describes prebuilt installation.

These commands are instructions for the caller, not installation authorization.
After installation, rerun the skill from this gate. A successful installation
still needs the freshness and schema checks below; a package version alone does
not prove compatibility.

## Load the latest upstream skill

Before planning or changing release inputs, fetch and read all four complete
resources from `folo-rs/folo`'s current `main` branch on every invocation.
Do not use a fixed revision or reuse a previous invocation's downloaded skill:

- [SKILL.md](https://raw.githubusercontent.com/folo-rs/folo/main/.github/skills/increment-versions/SKILL.md)
- [commands.md](https://raw.githubusercontent.com/folo-rs/folo/main/.github/skills/increment-versions/commands.md)
- [determining-level.md](https://raw.githubusercontent.com/folo-rs/folo/main/.github/skills/increment-versions/determining-level.md)
- [LICENSE](https://raw.githubusercontent.com/folo-rs/folo/main/.github/skills/increment-versions/LICENSE)

Follow the upstream procedure, command reference, and decision guide together.
Resolve their relative links against that upstream directory, not this local
wrapper. Retrieve any truncated content before proceeding. If retaining local
copies, keep the complete directory, including its license, outside the
repository or in an already ignored location.

Network access is required. If a resource is unavailable, incomplete, or
restricted, stop and report the blocker. Do not substitute remembered
instructions, an abbreviated procedure, another revision, or a link without
reading its content.

## Prerequisites and inputs

Use PowerShell **7.6** or newer for the upstream command examples. Require Git,
the selected Cargo/Rust toolchain, `cargo-release-plan`, and
`cargo-semver-checks` on `PATH`. Install or upgrade tools only with the applicable
permission. Anvil manages Rust verification tools, but its setup does not install
`cargo-release-plan`.

Require the latest published stable `cargo-release-plan` release. After the
prerequisite gate passes, query crates.io for that version and compare it with
the installed application version reported by `cargo release-plan version`.
If the installed tool is outdated, reject this invocation and provide the
unversioned install/upgrade command above. If release discovery fails, report
the blocker rather than assuming the installed version is current.

Follow the upstream
[installation guide](https://folo-rs.github.io/folo/cargo-release-plan/integration/installation.html).
Do not assume the latest action release, online book, and skill all use the same
interface. Before assessment, compare `cargo release-plan version` with the
schema requirements and any additional prerequisites in the freshly fetched
upstream skill. Do not hard-code schema revisions in this wrapper. Tool and
skill package versions need not match.

Require the explicitly selected workspace and local release-history ref, plus
the actual parent target ref for a stacked PR, or confirmation that there is no
unmerged parent. Ask for missing inputs rather than inferring them from publishing
configuration. Refresh remote refs only when requested. Work on a named feature
branch, not its merge destination.

Existing tracked edits need not be committed first. Track newly created intended
release inputs before assessment, staging only those paths and preserving
unrelated changes.

## Repository boundaries

Use only the upstream standalone path. Do not add publication configuration,
registry/publication gates, Trusted Publisher setup, or publishing workflows.
Preserve existing publication automation: merging a version PR can still trigger
it. This skill grants neither merge nor publication authority.

API comparisons may access published crates; standalone mode is not an offline
guarantee. Missing comparisons are not compatibility passes.

Preserve package include allowlists and exact dependency requirements. Keep
Anvil's external-type checks: the planner reads `allowed_external_types` but
does not verify those declarations against compiled APIs. Published implementation
packages are not automatically private APIs; do not add `private-api = true`
merely to bypass a compatibility finding.

Retain required `Cargo.lock` changes and run applicable Anvil checks after
upstream verification. Keep generated README files generated, and do not update
historical changelogs unless explicitly requested.

Include the upstream procedure's complete **Version/release plan** in the PR,
including dependent movements, alignment-only packages, retained increments,
and incomplete comparisons. State when nothing needs releasing. Keep all
assessment evidence out of commits, outside the repository or in an already
ignored location. Do not claim readiness when tools or evidence are incomplete.

## Upstream evolution

Follow upstream `main` and the latest published stable CLI without repository
version pins. If they are incompatible, stop and report the mismatch rather than
falling back to an older skill or tool. Retain the upstream license with downloaded
copies. Do not replace this local policy wrapper or switch modes to bypass a
failed check.
