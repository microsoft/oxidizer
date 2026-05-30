# AI Agents Guidelines

Code in this repository should follow the guidelines specified in the [Microsoft Rust Guidelines](https://microsoft.github.io/rust-guidelines/agents/all.txt).

## README Files

Crate README files are auto-generated via `just readme`. Do not manually update them.

## Executing `just` commands

If you only touch one crate, you may use `just package=crate_name command` to narrow command scope to one crate.

## Pre-commit Checklist

- Run `just format` to format code.
- Run `just readme` to regenerate crate-level readme files.
- Run `just spellcheck` to check spelling in code comments and docs.

## Spelling

The spell checker dictionary is in the `.spelling` file, one word per line in arbitrary order.

## Changelogs

The changelogs are updated by `scripts/release-crate.ps1` at release time, based on Git history. It is not necessary to make manual edits
to the changelogs, though you are permitted to do so if explicitly instructed.

## Release Versioning Vocabulary

When working on release tooling (`scripts/release-crate.ps1` and the helpers
in `scripts/lib/`), keep two vocabularies strictly separate:

- **Change types** describe the *semantic intent* of a release:
  `breaking` / `non-breaking` / `patch`. This is what the user reasons about
  and what the `release-crate.ps1` CLI accepts via
  `-Change Breaking|NonBreaking|Patch|1.0`. All user-visible output should
  use change-type vocabulary.
- **Version components** are *positions* in the SemVer string
  `major.minor.patch` (the three integers in `x.y.z`). These names are
  positional, not semantic. Do not call a `0.4.1 -> 0.5.0` increment a
  "major version change" — the value of the *major component* (0) did not
  change, even though the change is breaking under Cargo's 0.x semver rules.

The internal `$ChangeType` parameter on `Get-NextVersion` /
`Update-PackageVersion` / `Invoke-ReleaseFlow` uses the string values
`'breaking' | 'non-breaking' | 'patch'`. These are CHANGE-TYPE values, NOT
version-component names. The mapping from change-type to which version
component actually increments depends on whether the current version is
`1.x.y`, `0.x.y` (x >= 1), or `0.0.x`. Always translate to change-type
vocabulary via `Get-ChangeTypeLabel` before emitting user-facing output;
never present a version-component name (`major`/`minor`/`patch`) as a
stand-in for the change type.

## Release Dependency Scan

`scripts/release-crate.ps1`'s post-release dependency-scan loop (which surfaces
modified-but-unreleased workspace packages for the user to review) operates on
two invariants — keep them intact when editing the relevant code in
`scripts/lib/release-flow.ps1` and `scripts/lib/releasing.ps1`:

1. **Upstream cascades never introduce items to the user-review queue.** A
   package that received only a cascade-applied version change (no pre-existing
   developer modifications) requires no user review — its version change is
   mechanical and follows directly from the released dependency. Such packages
   must not surface in the dep-scan prompt. The implementation upholds this by
   snapshotting the "has unreleased modifications" set BEFORE the primary
   release / cascade runs so the snapshot reflects pre-cascade reality.
2. **A release-set member is removed from the user-review queue only when its
   cascade-applied change type is already at the semantic maximum (breaking).**
   If a release-set member has pre-existing developer modifications AND its
   cascade-applied change type is less than breaking (non-breaking or patch),
   the user must still be prompted because they may want to escalate the change
   type after reviewing the changes. Only when the change type is already
   breaking (no higher change type exists) can the member safely drop from the
   queue.

## Pull Requests

Pull request titles must follow [Conventional Commits](https://www.conventionalcommits.org/) naming, e.g. `feat(bytesbuf): add new metric` or `fix(cachet): correct eviction logic`.

## Feature-gated Doctests

Doctests that reference items behind a Cargo feature must compile both with and without that feature; wrap their bodies in hidden `#[cfg(...)]` shims. See [AGENTS-feature-gated-doctests.md](AGENTS-feature-gated-doctests.md).

## Required CI Checks

The `required-checks` job in `.github/workflows/main.yml` is a "fan-in"
aggregator: branch protection requires only this single context for jobs
defined in that workflow, and it succeeds when every dependency either
succeeded or was skipped.

When you add a new job to `main.yml`, you MUST also add it to the `needs:`
list of `required-checks` if it has BOTH a `strategy.matrix` AND a
job-level `if:` that can evaluate to false (typically gated on
`needs.delta.outputs.skip` or `github.event_name`). GitHub Actions does
not expand the matrix when such a gate skips the job, so per-OS contexts
like `testing (ubuntu-latest)` are never posted and would stay stuck on
`Expected — Waiting for status to be reported` if required directly.

Other required jobs should also be funnelled through `required-checks`
so branch protection only references one workflow context. See the
inline comment on the `required-checks` job for the full policy.

## Maintainability

While it is fine to use `.expect()`, the precondition is that it is either a programming error (the caller did something wrong)
or a situation that can never happen (in the absence of bugs). The expect-message must document either what the caller did wrong
in their code or why we believe the situation could never happen.

This is bad code: `self_span.get(self_offset..).expect("self_offset out of bounds")` - it does not explain what the caller did
wrong and it does not explain why we believe this access can never be out of bounds.

This is good code: `self_span.get(self_offset..).expect("guarded by min() above to never exceed span length")` - this explains
why we believe the operation can never cause an out of bounds access.
