# Scenario coverage

Executable scenarios live in
`scripts/tests/Pester/unit/releasing/ReleasePlan.Tests.ps1`.

The matrix covers:

| Area | Cases |
|---|---|
| Version lines | stable, `0.x`, `0.0.x` |
| Change types | breaking, nonbreaking, patch |
| Package state | previously released, first release, test-only first-release rejection, unpublished |
| Graphs | single, linear, diamond, duplicate normal/build edge, transitive exposure |
| Exposure | exposed, encapsulated, wildcard/unknown, empty/missing, stale roots |
| Proc-macros | implementation dependency, compatible/breaking contract, public/private use, major pin, generated runtime, blocked review |
| Compile evidence | partner-owned fixtures, pass→fail/fail→pass/unchanged floors, unmeasured and inconclusive blocks, sibling expectation discharge, selection-reason coupling, published-dependency fixtures |
| Regression evidence | fail→pass release, missing/pass→pass/fail→fail/pass→fail blocks, all three probe kinds, incomplete and self-contradicting measurements, single-revision probes, malformed entries, unaffected reasons |
| External dependencies | compatibility-line breaks, in-line bumps, exposed/private/proc-macro-only exposure, workspace-inherited candidate promotion, unknown exposure and unparsable requirements failing closed, classification and selection floors |
| Pins | valid, first release, build metadata, equal/downgrade rejection, satisfied cascade, conflict, force |
| Modes | targeted, changed, all, complete selection decisions, token consistency |
| Output | topological order, merged reasons, breaking flags, warnings |
| Changelogs | maintenance, breaking, multiple sorted reasons |
| Cargo APIs | internal edit, addition, removal, signatures, fields, traits, enums |
| Atomic apply | exact version edits, validation, rollback |

The test matrix is the hard oracle for mechanical behavior. Diff interpretation,
proc-macro semantics, and evidence-based elevation remain judgment-dependent and
must pass the multi-model consensus gate.

Selection review also covers generated README/changelog exclusion,
test/benchmark/dev-dependency-only declines, runtime dependency-feature patch
seeds, and baseline-pass/current-fail proc-macro fixtures. Fixture coverage is
mechanical: `release-facts.ps1` enumerates the changed compile fixtures in each
macro's review scope, and the resolver refuses any verdict weaker than the
outcomes measured for them. Selection reasons are held to the same standard: a
`behavior-fix` accept must exhibit a probe that failed at the release baseline
and passes now, so an internal adaptation cannot seed a release by being
described as a fix. External dependency requirements are read from cargo's own
resolved metadata on the current side and from the baseline manifests plus
`[workspace.dependencies]` on the other, so an inherited root bump is attributed
to every crate that inherits it and cannot be released as a patch while the
crate's public API exposes that dependency's types.
