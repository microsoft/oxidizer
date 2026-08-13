# Scenario coverage

Executable scenarios live in
`scripts/tests/Pester/unit/releasing/ReleasePlan.Tests.ps1`.

The matrix covers:

| Area | Cases |
|---|---|
| Version lines | stable, `0.x`, `0.0.x` |
| Change types | breaking, nonbreaking, patch |
| Package state | previously released, first release, unpublished |
| Graphs | single, linear, diamond, duplicate normal/build edge, transitive exposure |
| Exposure | exposed, encapsulated, wildcard/unknown, empty/missing, stale roots |
| Proc-macros | direct release, cascade, breaking exposure, manual review |
| Pins | valid, first release, build metadata, equal/downgrade rejection, satisfied cascade, conflict, force |
| Modes | targeted, changed, all |
| Output | topological order, merged reasons, breaking flags, warnings |
| Changelogs | maintenance, breaking, multiple sorted reasons |
| Cargo APIs | internal edit, addition, removal, signatures, fields, traits, enums |
| Atomic apply | exact version edits, validation, rollback |

The test matrix is the hard oracle for mechanical behavior. Diff interpretation,
proc-macro semantics, and evidence-based elevation remain judgment-dependent and
must pass the multi-model consensus gate.
