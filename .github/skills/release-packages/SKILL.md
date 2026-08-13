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
  modification state, and public exposure edges.
- `.github/skills/release-packages/scripts/resolve-plan.ps1` performs token
  parsing, version arithmetic, pins, exposure-aware cascades, and topological
  ordering.
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
   - Review proc-macro diffs manually and retain `manualReview: true`.
   - Review modified packages according to the elevation rules in
     `references/planning.md`.
   - If resolution reports missing classifications, classify the complete
     dependent closure it names and rerun with the same frozen facts.

4. **Resolve mechanically**
   - Write request JSON containing `mode`, accepted `tokens`, `classifications`,
     and optional `force`.
   - Run `scripts/resolve-plan.ps1 -FactsPath <facts.json> -RequestPath
     <request.json>`.
   - Treat its release set, versions, cascade reasons, and ordering as canonical.
   - In changed or all mode, stop without invoking the resolver when every
     reviewed candidate is declined.

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
   - Run `scripts/apply-plan.ps1 -PlanPath <plan.json>`.
   - Never reproduce its writes or rollback behavior by hand.

7. **Report**
   - Emit the canonical JSON plan and a concise table.
   - Include manual-review flags, warnings, and consensus status.

Do not publish packages unless the user explicitly requests publication.
