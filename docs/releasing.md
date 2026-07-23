# Releasing Oxidizer Packages

This document is the reference for the human-driven release tooling in
`scripts/`:

- `scripts/release-packages.ps1` — interactive release driver. Picks one
  of three mutually-exclusive target-selection modes:

  - `-Packages '<name>@<change-spec>', ...` — the caller supplies the
    full release plan up-front as `name@change-spec` tokens.
  - `-Changed` — guided walk through every workspace package with
    unreleased modifications (changes newer than the package's last
    `version =` / `publish =` commit). The script prompts for a
    per-package decision (view diff / ignore / release as breaking,
    non-breaking, or patch). Note: the change scan only sees files
    under `crates/<package>/`; modifications elsewhere in the
    repository (e.g. the workspace-level `Cargo.toml`, `.cargo/`,
    `deny.toml`, or shared CI configuration) do NOT surface a package
    even if they affect how it builds or behaves — use `-All` or
    `-Packages` to cover that case.
  - `-All` — guided walk through every publishable workspace package,
    even ones with no on-disk modifications. Use to force-walk the
    workspace when a refactor may have touched packages the change scan
    misses, or to coordinate a multi-package release after an internal
    cleanup.

  All three modes are interactive — even `-Packages` may prompt for
  elevation review when modified-but-unreleased dependencies of the
  requested packages are detected. The script must be run from an
  interactive terminal.

  In every mode the same pipeline runs: plan resolution,
  cascade toward dependents, an interactive elevation review for any
  modified-but-unreleased dependencies, a final plan display, then
  atomic application of all version-number increments, changelog
  updates, README regeneration, and `Cargo.toml` rewrites.

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
  dependents.

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
published dependent as a cascade-source release. For ordinary library
packages, the required change type — both for directly-requested
(user-source) packages and cascade-pulled dependents — is derived by running
[`cargo semver-checks`](https://crates.io/crates/cargo-semver-checks)
against each crate's **previous version-bump commit in git history** —
the most recent commit that changed the crate's `[package] version`,
supplied to the tool as `--baseline-rev <sha>`. cargo-semver-checks
rebuilds the baseline rustdoc from the crate's source at that commit, so
**no registry access is required** and the check behaves identically for
open-source (crates.io) and enterprise/offline consumers. The current
working-tree API is analysed, so a coordinated release's in-progress
edits — including a dependency whose public types a dependent re-exports
— are reflected in the dependent's own API diff.

Versioning is treated as a **source-level** concern: the baseline is the
version the repository last *declared*, regardless of whether it was ever
published anywhere. This is what lets one workflow serve both public and
private/enterprise environments (which cannot reach crates.io and whose
published content lags the source), and it means an aborted release that
bumped a crate to `4.0.0` without publishing is still the baseline the
next change is measured against.

This replaces the former
`[package.metadata.cargo_check_external_types]` allowlist heuristic. That
allowlist is a hand-maintained list of the external types a crate is
*permitted* to expose; it was repurposed as a proxy for the types a
crate *actually* re-exports. When the two drift apart — an entry missing
or stale — the heuristic misjudged whether a dependent re-exports a
changed dependency, so a breaking change in an exposed dependency could
be cascaded as `patch` instead of `breaking` (the motivating defect: a
breaking change in `bytesbuf` was not propagated to `bytesbuf_io`, which
re-exports `bytesbuf` types). Analysing the real API with
`cargo semver-checks` removes the proxy entirely.

**How the change type is determined.** `cargo semver-checks` is invoked
as a CLI (not as a library) and its textual result is parsed into one of
our change types. The mapping mirrors the tool's own
[`required_bump`](https://docs.rs/cargo-semver-checks/latest/cargo_semver_checks/struct.CrateReport.html#method.required_bump)
notion (major / minor / none); the exact parsing lives in
`ConvertFrom-SemverChecksOutput` (`scripts/lib/releasing.ps1`):

| `cargo semver-checks` result | change type |
|---|---|
| a major-level change is required | `breaking` |
| only a minor-level change is required | `non-breaking` |
| compatible / no update required | `patch` |
| no prior version-bump commit (new crate) | no constraint |

Cascade dependents are floored at `patch` (they must re-release to pick
up the new dependency version even when their own public API is
unchanged), then raised to whatever their own `cargo semver-checks`
result requires.

#### Proc-macro-only packages require manual SemVer review

`cargo semver-checks` deliberately supports ordinary library targets,
not proc-macro-only targets. For a package whose `cargo metadata`
targets contain `proc-macro` but no ordinary `lib` target, the tool exits
with "no crates with library targets selected". This is expected: its
rustdoc-based analysis cannot validate the procedural macro contract,
including exported macro names, accepted input syntax, diagnostics, or
generated output.

The release tooling detects this target shape from the workspace
metadata **before invoking `cargo semver-checks`**. It does not reinterpret
the unsupported-tool error as success and does not guess a breaking
change:

- Every proc-macro-only package in the release set is shown in the
  standard interactive package dialog, even when it was supplied via
  `-Packages` or was cascade-added without changes in its own folder.
- The dialog uses the same renderer, choices, and decision handling as
  every other package. Proc-macro detection only bypasses the unsupported
  automated check and records that the standard decision was made
  manually.
- Use **View diff**, then either keep the currently planned change type
  or select breaking / non-breaking / patch. For a targeted package, a
  new selection replaces the provisional `-Packages` change type. For a
  cascade-added package, it replaces the mechanical `patch` floor.
- The final release plan labels the package as manually classified and
  states that `cargo-semver-checks` was not run for it.
- Ordinary library dependents keep their normal behavior: each is
  re-released at least as `patch`, and its own public API is still
  analysed by `cargo-semver-checks`. A manually chosen proc-macro
  severity is never copied to dependents.
- When the reviewed proc-macro release is breaking, its direct published
  consumers receive an additional mandatory manual-review dialog. Their
  provisional level remains `max(patch, cargo-semver-checks result)`.
  A final increment that is non-breaking under that package's SemVer
  rules stops manual propagation there. A breaking increment advances
  the same review to that package's direct published consumers. This
  repeats one dependency edge at a time, while all other transitive
  dependents stay on the normal cascade path. For `0.0.x` packages,
  every increment is breaking and therefore advances review.

The CI SemVer report follows the same target detection. It skips the
unsupported invocation, emits a `warn` row saying manual proc-macro
review is required, and does **not** claim that the version increment was
automatically verified. For a breaking proc-macro increment, CI marks
the direct published consumer for manual review while retaining that
consumer's ordinary `cargo-semver-checks` result. It continues only
through consumers whose own version increment is breaking. If a required
direct consumer is absent from the publishing set, the report calls out
the incomplete review chain. If CI cannot determine a reviewed package's
baseline, it conservatively continues the warning to the next edge rather
than treating the unknown result as non-breaking.

Build and test validation are separate from SemVer validation. The
release driver runs `cargo check --workspace` after applying the plan,
and normal CI exercises the workspace tests. Those checks can catch
compilation failures and tested behavioral regressions, but passing them
does not prove compatibility for exported macro names, all accepted
inputs, diagnostics, or generated code. Review those aspects explicitly.

For example, to validate the main consumer and release
`templated_uri_macros`, run:

```powershell
cargo test -p templated_uri
./scripts/release-packages.ps1 -Packages 'templated_uri_macros@patch'
```

The `patch` token is the provisional plan entry, not an automated
compatibility verdict. In the standard package dialog, view the diff
and choose the actual change type before allowing the release to proceed.
If that choice is breaking, the planner next requires review of
`templated_uri`, its direct published consumer; keep or elevate
`templated_uri` based on whether its public contract exposes the macro
change.

**Baseline semantics.** The baseline is the crate's previous
version-bump commit — the most recent commit (before the change under
review) that altered the crate's `[package] version`. Because it comes
from git history rather than a registry, a version that was committed but
never published *is* the baseline: an aborted release that bumped
`bytesbuf` to `4.0.0` without publishing means the next change is
compared against `4.0.0`, not a stale published `3.3.3`. A brand-new
crate with no prior version-bump commit has no baseline and imposes no
constraint. This works offline and in enterprise environments with no
crates.io access, since the baseline API is rebuilt from the crate's own
source at the baseline commit.

The planner enforces **topological consistency**: if a user-supplied
change type for a package is *weaker* than `cargo semver-checks`
requires (for that package or via a cascade), the planner auto-upgrades
it and notes the upgrade in the review output. The caller's `-Packages`
token is therefore a *lower bound*, not a guarantee — the caller can
always elevate further on the next iteration of the review, but cannot
suppress a change type the API analysis requires.

### Errors the planner rejects

- An explicit semver that is not strictly greater than the package's
  current on-disk version. (Always fatal — `-Force` does not relax this.)
- A user-supplied change type that pins the package *below* what
  `cargo semver-checks` (or the cascade) computes for it. (The planner
  can auto-upgrade ordinary change-type tokens, but treats an explicit
  semver token as a hard pin — if the explicit version is below what the
  analysis requires the planner errors instead of silently overriding
  the caller. Pass `-Force` to override: the pin is honored verbatim, the
  package's effective change-type tag is still upgraded so further
  cascade decisions are correct, and a warning is printed flagging that
  consumers may break.)

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
   explicit version. Picking too weak a change type causes consumers
   to silently get incompatible behaviour after `cargo update`;
   picking too strong a change type is harmless except it forces direct
   dependents to bump as well.

2. Run one of:

   ```powershell
   # Targeted — pin the plan up front:
   ./scripts/release-packages.ps1 -Packages 'pkg1@<change-spec>','pkg2@<change-spec>'

   # Guided walk through every package with on-disk modifications:
   ./scripts/release-packages.ps1 -Changed

   # Guided walk through every publishable package, modified or not:
   ./scripts/release-packages.ps1 -All
   ```

   The script will:
   - Parse the tokens (targeted mode) or seed the review loop with every
     modified / every publishable package (guided modes) and compute the
     resolved release set, including the cascade toward dependents.
   - Show the release plan.
   - For each workspace package with unreleased modifications that is
     transitively pulled in by something in the release set (and is not
     itself in the release set with a cascade-applied change type of
     `breaking`), show a per-package menu where you can view the diff and
     decide whether to include the package, elevate its change type, or
     leave it out. In `-All` mode the same menu is also shown for
     publishable packages with no on-disk changes; the "View diff" option
     is relabelled `View diff (no changes in this package)` so the empty
     state is obvious before you open the editor.
   - For every proc-macro-only package in the release set, show the same
     menu as a mandatory manual SemVer review. This includes targeted
     packages and unchanged proc-macro dependents added by cascade.
     A breaking result then surfaces direct published consumers one edge
     at a time; propagation stops at the first consumer reviewed below
     breaking.
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
state first, then re-run with the corrected arguments.

---

## Guided modes (`-Changed`, `-All`)

Both `-Changed` and `-All` walk the workspace one package at a time and
prompt for a per-package release decision. They differ only in which
packages get surfaced:

- `-Changed` surfaces every published workspace package with unreleased
  modifications. Use this when you know "something changed and probably
  needs releasing" but do not yet have the full `-Packages` list ready.
  If the scan finds no packages with unreleased modifications, the
  script prints a confirmation and exits without prompting.

  The change scan only inspects files under `crates/<package>/`. Edits
  to anything outside a package directory — the workspace-level
  `Cargo.toml`, `.cargo/`, `deny.toml`, shared CI workflows, top-level
  scripts — are invisible to the scan even if they affect how the
  package builds or behaves. Switch to `-All` (or pass the affected
  packages with `-Packages`) when a cross-cutting change matters.

- `-All` surfaces every published workspace package, regardless of
  whether the on-disk content has been modified. Use this when you want
  to force-walk the entire workspace — for example to coordinate a
  multi-package release after an internal refactor, or when you suspect
  the change scan might be missing something. Packages with no detected
  changes still expose the View-diff option (relabelled to make the
  empty state obvious) so muscle-memory navigation continues to work.

For each surfaced package the menu lets you:

- **View the diff** since the last release commit.
- **Ignore** the package (leave it unreleased; treat the change as
  immaterial or not yet ready).
- **Release as breaking / non-breaking / patch** — synthesises a release
  token for the package internally and feeds it back into the planner.

Acceptances behave exactly as if you had passed the corresponding
`-Packages` token: the planner re-resolves the release set, computes
the cascade toward dependents, and the next iteration surfaces any
newly-relevant elevation candidates. Decisions are final — each
package is prompted at most once. If a later acceptance cascade-pulls
a previously-ignored package into the release set, or strengthens an
already-reviewed package's cascade level, the planner silently accepts
the cascade-applied level (reflecting the user's earlier decision not
to elevate). The final release plan summary records the cascade
reasons for every released package.

Conceptually, both guided modes are equivalent to imagining a virtual
`*` package that depends on every surfaced workspace package and
running the planner to cascade releases from `*` outward. There is no
real `*` token; the review loop seeds its dependency BFS with every
surfaced package as an additional root, so per-package chains between
surfaced packages emerge naturally during planning.

For each surfaced package the menu lists **every in-workspace dependency
chain** ending at that package — not only the chains rooted at the
current release set. This gives the reviewer a release-set-independent
big-picture view of what releasing the package could ripple through
(cascading may pull more dependents into the release set after the
prompt, so a release-set-rooted listing would be misleadingly narrow).
A package with no in-workspace dependents is shown with the hint
"No in-workspace dependents".

If you skip every prompt, the script exits without writing any files.

---

## Why we say "package" everywhere

Cargo's official term for a workspace member is "package", so the
release tooling uses "package" throughout the PowerShell API surface
(`-Packages`, `-PackageName`, etc.) and in all human-readable output.

The token "crate" survives only in identifiers carried over from
Cargo's own vocabulary — the filesystem directory `crates/`,
`[workspace.dependencies]`, `Cargo.toml`, `cargo metadata`, `crates.io`.
