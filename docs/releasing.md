# Releasing Oxidizer Crates

This document is the reference for the human-driven release tooling in
`scripts/`:

- `scripts/release-crate.ps1` — interactive release driver for a single
  workspace package.
- `scripts/check-unreleased-dependencies.ps1` — CI helper that flags for
  reviewer attention any workspace packages with unreleased modifications
  that are transitively pulled in by a package this PR is releasing.
- `scripts/lib/releasing.ps1`, `scripts/lib/release-flow.ps1` — library
  helpers dot-sourced by the entry-point scripts. Not direct entry points.

Maintainers SHOULD read the **Glossary** below before making changes to
the release tooling; the rest of the codebase, the PR comments, the
script output, and the unit tests all use these terms with the precise
meanings defined here.

---

## Glossary

- **Direct dependency** — a workspace package listed under another
  package's `[dependencies]` (or `[dev-dependencies]` /
  `[build-dependencies]`) in its `Cargo.toml`. If `bytesbuf_io` lists
  `bytesbuf`, then `bytesbuf` is a direct dependency of `bytesbuf_io`.

- **Transitive dependency** — a workspace package reachable through
  some chain of direct-dependency edges. Every direct dependency is also
  a transitive dependency.

- **Direct dependent** — the inverse of direct dependency. If
  `bytesbuf_io` lists `bytesbuf`, then `bytesbuf_io` is a direct
  dependent of `bytesbuf`.

- **Transitive dependent** — the inverse of transitive dependency: any
  package reachable through a chain of dependent edges. Every direct
  dependent is also a transitive dependent.

  > Avoid "upstream" / "downstream" — they are ambiguous (their meaning
  > depends on which way the reader visualises the graph). Always use the
  > dependency/dependent vocabulary above.

- **Cascade toward dependents** — the automatic version-number-increment
  propagation that happens when a released package's transitive
  dependents need to also be released because they (transitively) consume
  it. This is what `Invoke-CascadeStep` performs after the primary
  release.

- **Cascade toward dependencies** — the inverse: re-running
  `release-crate.ps1` on a package whose direct dependencies still have
  unreleased modifications causes the user to be prompted, and may
  trigger releases on those dependencies.

- **Change type** — the *semantic intent* of a release:
  `breaking` / `non-breaking` / `patch`. This is what releasers reason
  about and what the `release-crate.ps1 -Change` parameter accepts (as
  `Breaking|NonBreaking|Patch|1.0`).

- **Version component** — a *position* in the SemVer string
  `major.minor.patch` (the three integers in `x.y.z`). These names are
  positional, not semantic. The same change type maps to different
  version components depending on the current version (see
  `Get-NextVersion` in `scripts/lib/releasing.ps1`):

  | Current   | Breaking      | NonBreaking      | Patch            |
  |-----------|---------------|------------------|------------------|
  | `x.y.z`, x≥1 | `(x+1).0.0` | `x.(y+1).0`     | `x.y.(z+1)`      |
  | `0.y.z`, y≥1 | `0.(y+1).0` | `0.y.(z+1)`     | `0.y.(z+1)`†     |
  | `0.0.z`      | `0.0.(z+1)` | `0.0.(z+1)`     | `0.0.(z+1)`      |

  † On `0.x.y` the menu hides the Patch option because it would produce
  the same numeric outcome as NonBreaking.

  Do not call a `0.4.1 → 0.5.0` increment a "major version change" — the
  value of the *major component* (0) did not change, even though the
  change is breaking under Cargo's 0.x SemVer rules. Always translate
  change-type values to a user-friendly noun phrase via
  `Get-ChangeTypeLabel` before emitting user-facing output.

- **Release set** — the set of workspace packages whose on-disk
  `version =` differs from the value in the base ref. This is the
  same set returned by `Get-PackagesWithVersionChanges`. A package is
  in the release set whether the version change is uncommitted,
  committed-but-not-yet-pushed, committed-and-pushed, or arrived via a
  cascade-applied edit — *anything* not yet merged to the base ref
  counts. The release set is the unit the release tooling promises to
  publish to crates.io when the PR merges.

- **Pending release** — a member of the release set whose version
  increment has not yet been merged to the base ref. Committed-vs-
  uncommitted is irrelevant: the release tooling treats both the same.

- **Elevation** — running `release-crate.ps1` again on a package that is
  already a pending release, with a stronger change type. The same
  package may have multiple invocations on a branch; only the final
  on-disk state matters. Use elevation when:

  1. A cascade applied a non-breaking or patch change type, but on
     review the package's own pre-existing modifications warrant a
     breaking release.
  2. You initially released a package as a patch, then later realised
     you should have released it as a breaking change.

  Elevation works the same way whether the prior version increment is
  committed or uncommitted on the branch — the script reads the base
  ref's version (not the on-disk version) to compute the new increment,
  so you cannot accidentally double-bump or get stuck.

  See `Update-PendingReleaseVersion` in `scripts/lib/release-flow.ps1`
  for the implementation. It re-stamps the package's `Cargo.toml`, the
  workspace `Cargo.toml`'s `[workspace.dependencies]` entry, and the
  CHANGELOG section header in place — it does NOT create a second
  changelog section.

---

## Cascade Organisation Invariants

`scripts/release-crate.ps1`'s post-release dependency-scan loop (which
surfaces modified-but-unreleased workspace packages for the user to
review) operates on two invariants. Keep them intact when editing
`scripts/lib/release-flow.ps1` and `scripts/lib/releasing.ps1`:

### Invariant A — A cascade toward dependents never introduces items to the user-review queue.

A package that received only a cascade-applied version change (no
pre-existing developer modifications) requires no user review — its
version change is mechanical and follows directly from the released
dependency. Such packages must not surface in the dep-scan prompt.

The implementation upholds this by snapshotting the
"has unreleased modifications" set BEFORE the primary release / cascade
runs, so the snapshot reflects pre-cascade reality.

### Invariant B — A release-set member drops from the user-review queue only when its cascade-applied change type is already at the semantic maximum (breaking).

If a release-set member has pre-existing developer modifications AND its
cascade-applied change type is less than breaking (non-breaking or
patch), the user MUST still be prompted because they may want to
elevate the change type after reviewing the modifications. Only when the
cascade-applied change type is already breaking (no higher change type
exists) can the member safely drop from the queue.

The user-review queue therefore contains two categories of finding:

- **Modifications not part of this release** — packages with
  modifications that are NOT in the release set. The user must decide
  whether the modifications warrant a release.
- **Elevation candidates** — packages with modifications that ARE in
  the release set but whose cascade-applied change type is not yet
  breaking. The user must decide whether to elevate.

---

## How to release a crate

Run `scripts/release-crate.ps1 -Name <pkg> -Change <Breaking|NonBreaking|Patch|1.0>`
locally. The script will:

1. Compute the new version from the change type and the base ref's
   version.
2. Update the package's `Cargo.toml`, `Cargo.lock`, `README.md`, the
   workspace `Cargo.toml`'s `[workspace.dependencies]` entry, and the
   `CHANGELOG.md`.
3. Run the cascade toward dependents to bump any transitive dependent
   whose API surface is affected (`Test-PackageExposesTarget`).
4. Run the post-release dependency scan, which prompts you about each
   workspace package with unreleased modifications that is transitively
   pulled in by something in the release set. For each finding you can:
   - View the per-package diff before deciding.
   - Ignore (leave unreleased — the reviewer will see it flagged in
     `check-unreleased-dependencies.ps1`'s comment).
   - Release as breaking / non-breaking / patch (which itself triggers
     a sub-cascade and may add more findings to the next iteration).

The `-Change` value is the caller's judgment. There is no algorithmic
"correct" answer — the author must review the actual diff being
released (source + dependency edits) and decide whether the cumulative
change is a breaking SemVer change, a backward-compatible addition, a
pure internal patch, or the one-time `0.x → 1.0` graduation. Picking too
weak a change type causes downstream consumers to silently get
incompatible behaviour after `cargo update`; picking too strong a change
type is harmless except it forces direct dependents to bump as well.

You may run `release-crate.ps1` multiple times on the same branch — each
invocation reads the base ref's version (not the on-disk version) to
compute the new increment, so elevation and additional package releases
both work cleanly whether prior version increments are committed or not.

---

## How `check-unreleased-dependencies.ps1` works

The check script runs in CI on every PR that touches `crates/` or
`scripts/`. It computes the same dep-scan analysis as the interactive
loop and posts a PR comment with two tables:

- **Modifications not part of this release** — packages with unreleased
  modifications transitively pulled in by something in the release set
  but NOT themselves in the release set. The author may have
  deliberately left them out because the modifications are immaterial;
  the comment is advisory only.
- **Elevation candidates** — release-set members with pre-existing
  modifications whose cascade-applied change type is less than breaking.
  Reviewers should confirm the cascade-applied change type is adequate.

To act on a finding, re-run `release-crate.ps1 -Name <pkg> -Change <...>`
locally. This works for both:

- **Releasing a previously-skipped package** — the new version increment
  is added to the release set on top of any existing cascade-applied
  changes.
- **Elevating an existing release-set member** — the cascade-applied
  version is re-stamped to the elevated change type via
  `Update-PendingReleaseVersion`.

Both cases work whether the prior cascade-applied version increment is
committed or uncommitted on the branch.

---

## Why crate-vs-package nomenclature is mixed

Cargo's official term for a workspace member is "package". The release
tooling uses "package" throughout the PowerShell API surface (`-Name`,
`-PackageName`, `Invoke-PackageRelease`, `Get-PackagesWithVersionChanges`,
etc.) and in all human-readable output.

The tokens "crate" / "crates" survive only where they are part of an
external identifier we cannot change:

- The filesystem directory `crates/`.
- The script filename `release-crate.ps1` (kept for muscle-memory; the
  parameter is `-Name`).
- `[workspace.dependencies]`, `Cargo.toml`, `cargo metadata`, etc.
- `crates.io`.
- The `-CrateName` alias on `-Name` (for muscle memory).
