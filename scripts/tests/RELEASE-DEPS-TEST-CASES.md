# Test cases for the release-deps automation

This document captures **manual test scenarios** for the upstream-dependency
analysis implemented in `scripts/lib/releasing.ps1` and exercised by both
`scripts/release-crate.ps1` (interactive flow) and
`scripts/check-unreleased-dependencies.ps1` (CI flow).

These are intended to be re-run by future maintainers or agents when:

- The release-deps logic is modified.
- A new edge case is reported.
- A regression is suspected.

There are no automated tests for this code yet — PowerShell harness setup is
non-trivial and the logic is deeply tied to git history shapes. Until that
infrastructure exists, this document is the source of truth for "what behaviour
should be preserved".

## Test fixture setup

Use a **scratch worktree**, never the live checkout, to avoid corrupting
state or commit history.

```powershell
$tmp = "$env:TEMP\release-deps-tests"
if (Test-Path $tmp) { Remove-Item $tmp -Recurse -Force }
cd <path-to-oxidizer>
git worktree add -B release-deps-tests $tmp HEAD
cd $tmp
```

Between scenarios, restore working-tree state with:

```powershell
git checkout -- crates Cargo.toml Cargo.lock     # keep scripts/ edits intact
git clean -fd crates                              # drop untracked synthetic files
```

When done:

```powershell
cd <path-to-oxidizer>
git worktree remove --force $tmp
git branch -D release-deps-tests
```

## How to interpret each scenario

For each scenario the columns are:

- **ID** — stable identifier; use these in commit messages.
- **Scenario** — the situation being created.
- **Expected** — the desired script output.
- **Result** — the most-recent observed result. Marker convention:
  - `✅` confirmed passing.
  - `⚠️` passed but with a caveat worth noting.
  - `❌` regressed; investigate before shipping.
  - Blank — not yet exercised on the current code revision.

When a code change lands, re-run the scenarios and refresh the **Result**
column. If you discover a new scenario, append it (don't renumber existing
ones; IDs are stable references).

## T-series — single-PR / base-ref-relative scenarios

The original implementation only detected modifications **within the current
PR** (diff vs `BaseRef`). These T-series scenarios cover that surface and were
all confirmed during the initial implementation.

> ⚠️ **Re-run caveat under the per-crate baseline logic.** Several T-series
> rows assert "NO findings". Those assertions assumed the old `BaseRef`-relative
> baseline (a clean PR diff produced an empty modified set). Under the new
> per-crate baseline, unrelated workspace crates that have outstanding
> unreleased changes on `main` (e.g. `ohno`, `ohno_macros`, `thread_aware`) may
> legitimately appear as findings *in addition* to the scenario-specific
> expectation. When re-running T7/T8/T11/T12/T14/T15, interpret "NO findings"
> as "**the scenario's named crate does not appear in the findings table**",
> not "the output file is empty". The N-series harness uses
> `Assert-NoFindingForCrate` to encode that semantic.

| ID  | Scenario                                                                                                       | Expected                                                                                                  | Result |
|-----|----------------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------|--------|
| T1  | Bump `bytesbuf_io`, modify only `crates/bytesbuf/CHANGELOG.md` (non-source)                                    | Flag `bytesbuf`, chain `bytesbuf_io→bytesbuf`                                                              | ✅      |
| T2  | Bump `bytesbuf_io`, add untracked `crates/bytesbuf/src/scratch.rs`                                             | Flag `bytesbuf`                                                                                            | ✅      |
| T3  | Bump `bytesbuf_io`, modify `ohno_macros` (chain through unchanged `ohno`)                                      | Flag `ohno_macros`, chain `bytesbuf_io→ohno→ohno_macros`                                                   | ✅      |
| T4  | Bump `bytesbuf_io`, modify both `ohno` AND `ohno_macros`                                                        | Both flagged with distinct chains                                                                          | ✅      |
| T5  | Manually bump `ohno` (release-set) + `bytesbuf_io`, modify `ohno_macros`                                       | Flag `ohno_macros` via `ohno`'s BFS only; `bytesbuf_io`'s BFS stops at `ohno`                              | ✅      |
| T6  | Bump `tick`, modify `ohno` (only dev-dep of tick)                                                              | (Cascade-expanded release set made this an unclean dev-dep test; superseded by T6b.)                       | ⚠️     |
| T6b | Bump leaf `seatbelt` (no cascade), modify `ohno` (dev-only of seatbelt)                                        | NO findings (confirms dev-dep exclusion)                                                                   | ✅      |
| T7  | Bump `bytesbuf_io`, modify only root files (README, justfile)                                                  | NO findings                                                                                                | ✅      |
| T8  | Bump `bytesbuf_io`, no other changes                                                                            | NO findings                                                                                                | ✅      |
| T9  | Interactive: 2 modified deps, accept `bytesbuf`, decline `ohno_macros`                                          | Re-prompts `ohno_macros` after `bytesbuf` release; decline honored                                          | ✅      |
| T10 | `check-unreleased-dependencies.ps1` with state {released `bytesbuf_io` + modified `bytesbuf`}                  | Markdown produced; LF line endings; sticky-comment shape correct                                            | ✅      |
| T11 | `check-unreleased-dependencies.ps1` against a clean tree                                                       | "No findings" output, no `OutputFile` created                                                              | ✅      |
| T12 | Empty release set (modified file but no version bumps)                                                         | "No findings"; no `OutputFile`                                                                             | ✅      |
| T13 | `-BaseRef origin/does-not-exist`                                                                                | Graceful warning + exit 0, no NRE                                                                          | ✅      |
| T14 | Bump `bytesbuf_io`, modify only `crates/bytesbuf_io/src/…` (released crate itself)                              | NO findings (released crate excluded from upstream check)                                                  | ✅      |
| T15 | Bump `bytesbuf_io`, modify `crates/automation/…` (`publish = false`)                                            | NO findings (`Published` filter)                                                                           | ✅      |
| T16 | Bump `bytesbuf` (cascades to 3 crates), modify `thread_aware` (dep of all 3)                                    | All 3 chains aggregated in a single finding                                                                | ✅      |

## N-series — multi-PR / since-last-release scenarios

These scenarios cover the gap that the **base-ref-relative** baseline missed:
modifications committed to `main` in earlier PRs (without a version bump) and
then depended on by a release-set crate in the current PR. The fix replaces
the global `BaseRef` baseline with a **per-crate** baseline computed as the
most recent commit that touched `version =` or `publish =` in that crate's
`Cargo.toml`.

To simulate a "previous PR that merged-but-didn't-release" change, commit edits
to the upstream crate on the branch base **before** the version-bump commit, so
that those edits are part of `BaseRef` history but newer than the upstream
crate's version-bump baseline.

| ID  | Scenario                                                                                                                                          | Expected                                                                              | Result |
|-----|---------------------------------------------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------|--------|
| N1  | Single-PR sanity: modify `bytesbuf` source + bump `bytesbuf_io` in the same PR; release set = `{bytesbuf_io}`                                     | Flag `bytesbuf` (same as T1 family, but via new logic)                                | ✅      |
| N2  | **User's case** — earlier commit on main modifies `bytesbuf` source (no bump); current PR commit only bumps `bytesbuf_io`                          | Flag `bytesbuf`                                                                       | ✅      |
| N3  | Earlier commit bumped `bytesbuf` cleanly (no further edits since); current PR bumps `bytesbuf_io`                                                 | No finding                                                                            | ✅      |
| N4  | Earlier commit bumped `bytesbuf`; later commit edited `bytesbuf` source (no bump); current PR bumps `bytesbuf_io`                                 | Flag `bytesbuf` (per-crate baseline = earlier bump commit; later edit is unreleased)  | ✅      |
| N5  | Chain `A → B (unchanged since baseline) → C (modified in earlier PR, no bump)`; release set = `{A}`                                                | Flag `C` (chain `A→B→C`)                                                              | ✅      |
| N6  | `bytesbuf` has only `CHANGELOG.md` changes since baseline (no `.rs` / no `Cargo.toml`)                                                            | Flag `bytesbuf` (humans-decide policy retained)                                       | ✅      |
| N7  | `bytesbuf` was `publish = false`, flipped to `publish = true` in a commit, edits *before* the flip exist                                          | No finding — baseline is the publish-flip commit; pre-flip edits aren't unreleased    | ✅      |
| N8  | Uncommitted working-tree edits to `bytesbuf` source (interactive flow simulation), no commits since last bump                                     | Flag `bytesbuf`                                                                       | ✅      |
| N9  | Untracked new files in `crates/bytesbuf/src/` (no commits, no working-tree-tracked edits)                                                          | Flag `bytesbuf`                                                                       | ✅      |
| N10 | New crate `newone` added in current PR, `publish = true`, depended on by released crate `A`                                                       | `newone` is in the release set (no `Cargo.toml` at base ref); BFS stops at it; no separate finding | ⏭️ (structural; not in harness) |

### Automated N-series harness

`N1`–`N9` can be replayed without any manual setup by running
`run-n-tests.ps1` from inside a scratch worktree. The harness:

- assumes the worktree HEAD is the commit it should reset to between scenarios,
- invokes `scripts/check-unreleased-dependencies.ps1` from
  `C:\Source\Oss\oxidizer3` (the development tree) so the latest analysis code
  is exercised regardless of the worktree's pinned commit,
- uses `Assert-NoFindingForCrate` rather than "output file absent" so unrelated
  pre-existing noise on `main` doesn't cause false failures.

If the harness file is preserved between sessions it lives under the agent's
session state at `files/run-n-tests.ps1`; the source-of-truth columns above
should be updated whenever it is re-run.

`N10` is intentionally skipped from the automated harness — it requires editing
the root `Cargo.toml`'s `members` list and synthesising a brand-new crate
manifest, which is more setup than the structural assertion warrants. The
behaviour is covered by `Get-CratesWithVersionBumps`'s "new crate → in release
set" rule (see `scripts/lib/releasing.ps1`).

## Phase-3-style cleanup checklist

After running scenarios:

1. `git status` on the worktree should be clean (or contain only scenario-specific edits).
2. `git --no-pager log --oneline HEAD~5..HEAD` should match the scratch branch's expected shape (no real-branch commits).
3. Remove the worktree and branch (see "Test fixture setup" above).
4. Refresh the **Result** column in this file with the observed outcomes.

## Known open question

If only `CHANGELOG.md` or auto-generated `README.md` files were touched in an
unreleased upstream dep, the change is by definition immaterial. The current
behaviour is to **still flag** such cases (humans decide); see N6. If that
policy is ever flipped to an auto-suppression, N6's expected result must flip
to "No finding" and a new scenario should be added that exercises a mixed
`CHANGELOG + src/` change.
