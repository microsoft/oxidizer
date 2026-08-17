---
name: release-packages
description: Plan and apply deterministic Oxidizer workspace package releases. Use for targeted, changed, or all-package releases; version bumps; dependency cascades; proc-macro review; changelogs; and release validation.
license: MIT
---

# Release Oxidizer packages

Create an exact release plan, verify it across independent reasoning models, and
apply it atomically.

## Required reading

1. Read `docs/releasing.md` for repository terminology and compatibility rules.
2. Read `references/planning.md` for classification, elevation, consensus, and
   apply rules.
3. Read `references/version-rules.md` before reviewing a plan.

## Deterministic helpers

Run these from the repository root:

- `.github/skills/release-packages/scripts/release-facts.ps1` gathers the
  workspace graph, release baselines,
  modification state, public type exposure, macro publication, implementation
  closures, and generated-runtime relationships.
- `.github/skills/release-packages/scripts/resolve-plan.ps1` performs token
  parsing, version arithmetic, pins, type- and macro-contract-aware cascades,
  ambiguity reporting, and topological ordering.
- `.github/skills/release-packages/scripts/apply-plan.ps1` performs version
  writes, changelog and README generation, validation, and rollback.
- `.github/skills/release-packages/scripts/release-changelog.ps1` writes one
  deterministic changelog.

Never reproduce their work by hand.

## Workflow

1. **Preflight**
   - Require PowerShell 7, Cargo, `cargo semver-checks`, and a clean baseline.
   - Record `git rev-parse HEAD` and `git status --porcelain`.

2. **Gather facts**
   - Run `.github/skills/release-packages/scripts/release-facts.ps1` and save its
     JSON.
   - Determine targeted, changed, or all mode.

3. **Classify**
   - For every ordinary, previously released library that may enter the plan,
     run `cargo semver-checks` against its `baselineSha`.
   - Review each affected proc-macro contract across its
     `macroImplementationClosure` and `macroRuntimePartners`.
   - Record a `macroContracts` attestation covering exported macros, accepted
     syntax, compile behavior, generated API, runtime paths, and hygiene.
   - Measure every fixture the facts list in `macroCompileFixtureChanges` by
     compiling it at `baselineRev` and at the current revision, and record each
     result in `macroContracts.<package>.compileEvidence`. The resolver derives
     the verdict floor from those measurements and blocks a weaker verdict.
   - Retain `manualReview: true`.
   - Review every candidate according to the deterministic selection table in
     `references/planning.md`. Record an evidenced `selectionDecisions` entry
     for every candidate; never omit a package because its changes look
     mechanical.
   - For a `behavior-fix` reason, measure a consumer-runtime, consumer-compile,
     or packaged-artifact probe at the release baseline and at the current
     revision, and record both runs in
     `selectionDecisions.<package>.regressionEvidence`. Only a baseline failure
     that now passes demonstrates the fix; a preserved-behavior refactor is
     `internal-only`.
   - Check `externalDepChanges` against `externalExposedDeps`. A breaking
     external requirement change on a publicly exposed dependency forces a
     `breaking` classification and a `breaking` selection reason; the resolver
     blocks anything weaker. Private external dependencies and proc-macro-only
     packages are unaffected.
   - Classify implemented source and verified consumer behavior. TODOs, design
     notes, and roadmap text are not compatibility evidence.
   - If resolution reports missing classifications, classify the complete
     dependent closure it names and rerun with the same frozen facts.
   - Invoke the resolver with the complete decision map even when every
     candidate is declined; the canonical result is an empty resolved plan.

4. **Resolve mechanically**
   - Write request JSON containing `mode`, accepted `tokens`,
     `selectionDecisions`, `classifications`, required `macroContracts`, and
     optional `force`.
   - Run `.github/skills/release-packages/scripts/resolve-plan.ps1 -FactsPath
     <facts.json> -RequestPath <request.json>`.
   - Treat its release set, versions, cascade reasons, and ordering as canonical.
   - If it returns `status: blocked`, review every package named in
     `ambiguities` and rerun with the same frozen facts. Never convert an
     unresolved macro contract into a conservative breaking guess.
   - In changed or all mode, do not apply an empty resolved plan.

5. **Consensus**
   - Freeze the facts, classifications, reviewed evidence, request, and resolved
     plan.
   - Ask at least two additional model families to review the classifications
     and verify that the resolved plan follows from them.
   - Do not ask models to independently redo version arithmetic or cascade
     resolution.
   - Stop with the structured ambiguity report from `references/planning.md` if
     classifications or rules diverge.

6. **Apply atomically**
   - Run `.github/skills/release-packages/scripts/apply-plan.ps1 -PlanPath
     <plan.json>`.
   - Never reproduce its writes or rollback behavior by hand.

7. **Report**
   - Emit the canonical JSON plan and a concise table.
   - Include manual-review flags, warnings, and consensus status.
   - Copy normalized release fields and resolver warnings directly from
     `plan.json`; keep environment or methodology notes separate.

Do not publish packages unless the user explicitly requests publication.
