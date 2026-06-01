# Releasing Oxidizer Packages

This document is the reference for the human-driven release tooling in
`scripts/`:

- `scripts/release-packages.ps1` — interactive release driver. The caller
  supplies the full release plan up-front via `-Packages` (a list of
  `name@change-spec` tokens); the script resolves the plan into a release
  set, surfaces any direct or transitive dependencies that have unreleased
  modifications for the caller to review, and applies the resulting
  version-number increments, changelog updates, and dependent cascade in
  one shot.
- `scripts/release-changed-packages.ps1` — guided counterpart to
  `release-packages.ps1` for when you do not yet know which packages to
  release. The script walks you through *every* workspace package with
  unreleased modifications, one prompt at a time, and lets you ignore
  each one or release it as breaking / non-breaking / patch. Each
  acceptance feeds into the same plan resolver and cascade-toward-dependents
  logic as the token-based flow. Interactive-only.
- `scripts/check-unreleased-dependencies.ps1` — CI helper that flags for
  reviewer attention any workspace packages with unreleased modifications
  that are transitively pulled in by a package this PR is releasing.

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
  it. The planner walks the user-supplied release plan, computes the
  transitive dependents of each user-source release, and adds
  cascade-source entries to the plan so the dependents are also released.

- **Cascade toward dependencies** — the inverse: when a package being
  released has direct dependencies with unreleased modifications, the
  release plan does NOT automatically pull them in. Instead the planner
  surfaces them to the caller during the review step, who decides
  whether to release them too or leave them out. The surfaced
  dependencies that the caller accepts join the release plan as ordinary
  user-source releases — there is no separate "dependency-cascade-source"
  release kind because the caller is always the decision-maker for any
  pulled-in dependency.

- **Change type** — the *semantic intent* of a release:
  `breaking` / `nonbreaking` / `patch`. This is what releasers reason
  about. In the `-Packages` tokens it appears as the part after `@`, e.g.
  `bytesbuf@breaking`.

- **Change spec** — the value of the part after `@` in a `-Packages`
  token. A change spec is either a change type (`breaking`,
  `nonbreaking`, `patch`) or an explicit semver version like `1.0.0` or
  `2.5.0`. Change types are translated into concrete versions using the
  version-increment rules below. Explicit versions pass through verbatim
  and must be strictly greater than the package's current on-disk
  version.

- **Version component** — a *position* in the SemVer string
  `major.minor.patch` (the three integers in `x.y.z`). These names are
  positional, not semantic. The same change type maps to different
  version components depending on the current version:

  | Current   | breaking      | nonbreaking      | patch            |
  |-----------|---------------|------------------|------------------|
  | `x.y.z`, x≥1 | `(x+1).0.0` | `x.(y+1).0`     | `x.y.(z+1)`      |
  | `0.y.z`, y≥1 | `0.(y+1).0` | `0.y.(z+1)`     | `0.y.(z+1)`†     |
  | `0.0.z`      | `0.0.(z+1)` | `0.0.(z+1)`     | `0.0.(z+1)`      |

  † On `0.x.y` a `patch` change spec produces the same numeric outcome
  as `nonbreaking`. The planner does not reject this — the caller may
  still record the intent as `patch` so it shows up that way in the
  release plan and commit message.

  Do not call a `0.4.1 → 0.5.0` increment a "major version change" — the
  value of the *major component* (0) did not change, even though the
  change is breaking under Cargo's 0.x SemVer rules.

- **Release set** — the set of workspace packages a single release will
  publish to crates.io. The local driver (`release-packages.ps1`)
  materialises it as the **resolved release set**: the caller's
  `-Packages` tokens plus everything pulled in by the cascade toward
  dependents. The CI helper (`check-unreleased-dependencies.ps1`) has
  no `-Packages` input, so it derives the same conceptual set from the
  diff against the base ref.

- **Pending release** — a member of the release set that has not yet
  reached crates.io. Committed-vs-uncommitted is irrelevant: a
  version-number increment sitting in your working tree, a
  committed-but-unpushed increment, and a merged-but-untagged increment
  all count the same.

- **Resolved release set** — the per-invocation, in-memory result of
  plan resolution. It is a hashtable keyed by package folder where each
  entry records the package's source (`user` or `cascade`), the
  effective change type (after cascade-driven upgrade), the effective
  target version, and the list of cascade reasons (which user-source
  releases caused the cascade). The resolved release set is the
  planner's source of truth for the rest of the run.

- **User-source release** — a release plan entry derived directly from a
  `-Packages` token, OR added by the caller's "release this" choice
  during dep-scan review. Either way, the caller explicitly asked for
  this release.

- **Cascade-source release** — a release plan entry added by the
  cascade-toward-dependents walk during plan resolution. The caller did
  not list this package in `-Packages` and did not accept it during
  dep-scan review; it was added because it is a transitive dependent of
  a user-source release.

---

## Bundled-input release model

Every invocation of `release-packages.ps1` describes a *complete release
plan*. The planner reads the entire plan up-front (the `-Packages`
argument is the entire input — there is no base ref), resolves the
cascade toward dependents, surfaces any modified-but-unreleased
dependencies for review, and then applies all version-number increments,
changelog updates, and `Cargo.toml` rewrites in one shot. A second
invocation is treated as a fresh, independent release plan — there is
no notion of "adding to a previous run".

Version arithmetic anchors on what is currently in each `Cargo.toml` on
disk. The planner does not consult `git` for prior versions; it
increments from the value it reads right now. Consequently, if you
re-run on the same branch after a prior run already increased a version,
the new run will increment *on top of* that change — see
[Re-running on the same branch](#re-running-on-the-same-branch).

If you need to re-plan (for example because you accepted a release
during review that you now want to remove), use `git reset` /
`git restore` to revert the on-disk state and re-run the script with
the corrected `-Packages` argument.

### `-Packages` token syntax

Each token has the form `<name>@<change-spec>`:

- `<name>` is the package name as it appears in `crates/<name>/Cargo.toml`.
- `<change-spec>` is one of:
  - `breaking`, `nonbreaking`, `patch` — the change type. The planner
    computes the target version from the package's current version on
    disk using the version-increment rules in the **Version component**
    glossary entry.
  - An explicit semver (e.g. `1.0.0`, `2.5.0`, `0.10.0`) — used
    verbatim. Must be strictly greater than the current on-disk
    version. There is no special handling for any particular version
    value; `1.0.0` is just another explicit pin.

Examples:

```powershell
# Single package, non-breaking change.
./scripts/release-packages.ps1 -Packages 'bytesbuf@nonbreaking'

# Two packages: one breaking, one patch.
./scripts/release-packages.ps1 -Packages 'bytesbuf@breaking','bytesbuf_io@patch'

# Pin one package to 1.0.0 and another to an explicit version.
./scripts/release-packages.ps1 -Packages 'foo@1.0.0','bar@2.5.0'
```

### Cascade-toward-dependents and topological consistency

After parsing the tokens, the planner walks the workspace dependency
graph forward from every user-source release and adds each transitive
published dependent as a cascade-source release. The cascade's change
type for each dependent is derived from whether the user-source release
exposes (in its public API) the cascaded-from package — exposing
cascades propagate the source's change type, non-exposing cascades drop
to `patch`.

The planner enforces **topological consistency**: if a user-supplied
change type for a package is *weaker* than the cascade would compute,
the planner auto-upgrades it and notes the upgrade in the review output.
The caller's `-Packages` token is therefore a *lower bound*, not a
guarantee — the caller can always elevate further on the next iteration
of the review, but cannot suppress a cascade-imposed change type.

### Errors the planner rejects

- An explicit semver that is not strictly greater than the package's
  current on-disk version.
- A user-supplied change type that pins the package *below* what the
  cascade computes for it. (The planner can auto-upgrade ordinary
  change-type tokens, but treats an explicit semver token as a hard
  pin — if the explicit version is below what the cascade requires the
  planner errors instead of silently overriding the caller.)

---

## Cascade Organisation Invariants

The dependency-scan loop (which surfaces modified-but-unreleased
workspace packages for the caller to review) operates on two invariants.

### Invariant A — A cascade-added release-set member is never itself surfaced as a finding in the dep-scan.

A package that received only a cascade-applied version-number change
(no pre-existing developer modifications) requires no caller review —
its version-number increment is mechanical and follows directly from
the released dependency. Such packages must not appear in the dep-scan
prompt.

Cascading toward dependents *does* enlarge the release set, which
enlarges the set of packages whose dependencies the dep-scan walks. So
a cascade can INDIRECTLY cause pre-existing modified packages — packages
that already had unreleased modifications, not the cascade-added
members themselves — to surface as new findings on a subsequent
iteration of the review loop. This is the desired behaviour: the
caller wants to know about every package whose modifications might
need a release, and the cascade just enlarged the relevant scope.

The implementation upholds the no-self-surface part of this invariant
by snapshotting the "has unreleased modifications" set BEFORE any
cascade runs, so the snapshot reflects pre-cascade reality.

### Invariant B — User-source releases are never surfaced; cascade-source releases are surfaced only when not already at `breaking`.

The dep-scan surfaces a release-set member only when both:

1. It is a cascade-source release (added by the cascade toward
   dependents, not by a `-Packages` token or a caller acceptance
   during a prior dep-scan iteration).
2. Its cascade-applied change type is not yet `breaking` (so there is
   room for the caller to elevate it).

A user-source release is the caller's final decision. It is never
re-prompted, regardless of its change type — the caller already chose
the change type, and to revise it they re-invoke the script with a
different `-Packages` token (after first reverting the on-disk state).
A cascade-source release already at `breaking` is also dropped: no
higher change type exists, so there is nothing for the caller to
elevate.

The user-review queue therefore contains two categories of finding:

- **Modifications not part of this release** — packages with
  modifications that are NOT in the release set. The caller must decide
  whether the modifications warrant a release.
- **Elevation candidates** — packages with modifications that ARE in
  the release set as cascade-source releases but whose cascade-applied
  change type is not yet `breaking`. The caller must decide whether to
  elevate.

---

## How to release one or more packages

1. Decide which packages you want to release and the change type for each.
   This is the caller's judgment. There is no algorithmic "correct"
   answer — review the cumulative diff being released (source + dependency
   edits) and decide whether each package's change is breaking,
   backward-compatible, a pure internal patch, or whether to pin to an
   explicit version. Picking too weak a change type causes downstream
   consumers to silently get incompatible behaviour after `cargo update`;
   picking too strong a change type is harmless except it forces direct
   dependents to bump as well.

2. Run:

   ```powershell
   ./scripts/release-packages.ps1 -Packages 'pkg1@<change-spec>','pkg2@<change-spec>'
   ```

   The script will:
   - Parse the tokens and compute the resolved release set, including
     the cascade toward dependents.
   - Show the release plan.
   - For each workspace package with unreleased modifications that is
     transitively pulled in by something in the release set (and is not
     itself in the release set with a cascade-applied change type of
     `breaking`), show a per-package menu where you can view the diff and
     decide whether to include the package, elevate its change type, or
     leave it out.
   - After review, apply all version-number increments, changelog
     updates, README regeneration, `Cargo.toml` rewrites, and workspace
     `[workspace.dependencies]` updates in one shot.

3. Commit the resulting changes and open a PR.

Once your PR is merged, automation tags the commit and pushes each
released crate to crates.io.

### Re-running on the same branch

You may run `release-packages.ps1` multiple times on the same branch,
but each invocation reads the *on-disk* version of every package (the
planner uses the version it finds in `Cargo.toml`, not the version in
any base ref). A second run therefore plans increments *on top of*
whatever the first run already wrote — typically not what you want
when re-planning the same release.

If a previous run produced changes you want to discard before
re-planning, use `git reset` / `git restore` to revert the on-disk
state first, then re-run with the corrected `-Packages` argument.

---

## Guided changed-packages workflow

When you know "something changed and probably needs releasing" but do not
yet have the full `-Packages` list ready, use the guided alternative:

```powershell
./scripts/release-changed-packages.ps1
```

The script takes no arguments. It scans the workspace for every package
with unreleased modifications and walks you through them one at a time.
For each surfaced package the menu is the same as the per-package review
menu used by `release-packages.ps1`:

- **View the diff** since the last release commit.
- **Ignore** the package (leave it unreleased; treat the change as
  immaterial or not yet ready).
- **Release as breaking / non-breaking / patch** — synthesises a release
  token for the package internally and feeds it back into the planner.

Acceptances behave exactly as if you had passed the corresponding
`-Packages` token to `release-packages.ps1`: the planner re-resolves the
release set, computes the cascade toward dependents, and the next
iteration surfaces any newly-relevant elevation candidates. Decisions
are final — each package is prompted at most once. If a later
acceptance cascade-pulls a previously-ignored package into the release
set, or strengthens an already-reviewed package's cascade level, the
planner silently accepts the cascade-applied level (reflecting the
user's earlier decision not to elevate). The final release plan summary
records the cascade reasons for every released package.

Conceptually, the workflow is equivalent to imagining a virtual `*`
package that depends on every changed workspace package and running
`release-packages.ps1` to cascade releases from `*` outward. There is no
real `*` token; the review loop seeds its dependency BFS with every
changed package as an additional root, so per-package chains between
changed packages emerge naturally during planning.

For each surfaced package the menu lists **every in-workspace dependency
chain** ending at that package — not only the chains rooted at the
current release set. This gives the reviewer a release-set-independent
big-picture view of what releasing the package could ripple through
(cascading may pull more dependents into the release set after the
prompt, so a release-set-rooted listing would be misleadingly narrow).
A package with no in-workspace dependents is shown with the hint
"no in-workspace dependents".

`release-changed-packages.ps1` is **interactive-only**. For
scripted / CI use, invoke `release-packages.ps1` with an explicit
`-Packages` list so the choices are explicit and auditable.

If the scan finds no packages with unreleased modifications, the script
prints a confirmation and exits without prompting. If you ignore every
prompt, the script exits without writing any files.

---

## How `check-unreleased-dependencies.ps1` works

The check script runs in CI on every pull request (the `release-deps`
job in `.github/workflows/main.yml` has no path filter — every PR pays
the cost of one dep-scan analysis). It computes the same dep-scan
analysis as the interactive loop and posts a PR comment with two
tables:

- **Modifications not part of this release** — packages with unreleased
  modifications transitively pulled in by something in the release set
  but NOT themselves in the release set. The author may have
  deliberately left them out because the modifications are immaterial;
  the comment is advisory only.
- **Elevation candidates** — release-set members with pre-existing
  modifications whose cascade-applied change type is less than breaking.
  Reviewers should confirm the cascade-applied change type is adequate.

To act on a finding, re-run `release-packages.ps1` locally with a
corrected `-Packages` argument that:

- Adds any previously-skipped package as a new token (the planner will
  fold it into the release set on top of any cascade-applied changes).
- Strengthens the change type for any elevation candidate (the planner
  re-stamps the cascade-applied version to match the elevated change
  type).

Because the planner reads on-disk state, you typically want to discard
the prior run's changes first via `git reset` / `git restore` and then
re-invoke from a clean tree. Re-running on top of existing on-disk
increments would compound version-number increments rather than replace
them — see
[Re-running on the same branch](#re-running-on-the-same-branch).

---

## Why we say "package" everywhere

Cargo's official term for a workspace member is "package", so the
release tooling uses "package" throughout the PowerShell API surface
(`-Packages`, `-PackageName`, etc.) and in all human-readable output.

The token "crate" survives only in identifiers carried over from
Cargo's own vocabulary — the filesystem directory `crates/`,
`[workspace.dependencies]`, `Cargo.toml`, `cargo metadata`, `crates.io`.
