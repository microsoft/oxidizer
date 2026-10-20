---
name: prepare-release
description: Prepare an Oxidizer release or version PR by coordinating standalone increment-versions, release changelogs, and fresh verification. Use when asked to cut a release, bump versions, or prepare release notes; never merges, tags, or publishes.
license: MIT
---

# Prepare an Oxidizer release

Use this skill as the human entry point for an on-demand version PR. It
coordinates the repository's two focused skills without replacing their
instructions:

- [`increment-versions`](../increment-versions/SKILL.md) owns assessment,
  semantic impacts, version selection, dependency resolution, application, and
  compatibility evidence.
- [`release-changelogs`](../release-changelogs/SKILL.md) owns package release
  entries for the final resolved plan.

## Inputs and boundaries

Require the intended feature branch, release-history ref, optional stacked parent
target, and explicit release date. Refresh named refs when requested and record
their resolved commits. Ask for missing or ambiguous inputs; do not substitute
the repository default branch or today's date.

Run `increment-versions` in **standalone mode**. This workflow prepares a
reviewable version PR only. It does not merge, tag, publish, configure publishing,
or authorize any of those actions. Existing Microsoft automation remains
responsible for publication after an authorized merge.

## Workflow

1. Follow `increment-versions` with the supplied standalone inputs through a
   reviewed and applied resolved plan. Retain its reports, plan, decisions,
   compatibility evidence, and **Version/release plan** handoff. Stop before its
   terminal commit, PR, or merge actions; this coordinator owns the remaining
   handoff and never delegates merge.
2. Run `release-changelogs` against that exact resolved plan and release date.
3. Because changelog edits changed the assessed checkout, return to
   `increment-versions` and rerun its evidence and verification stages end to end
   in new evidence directories. Re-resolve the original refs first. If either
   moved, follow the component skill's recovery procedure and restart; preserve
   changelog edits and unrelated work.
4. If the fresh resolved package/version set differs, rerun
   `release-changelogs` for the new final plan and repeat step 3. Stop only when
   the resolved plan, applied manifests and lockfile, and changelog entries agree.
   Apply again only when the new plan differs from the applied manifests, and
   only after verifying that the pending delta is exactly that plan.
5. Stop uncommitted on any changelog blocker, partial application, failed check,
   or evidence mismatch. Otherwise run the smallest relevant documentation and
   repository checks, commit only the intended version, requirement, lockfile,
   and changelog changes, and create or update the ready-for-review PR when
   requested. Never commit generated evidence or unrelated edits.

## Handoff

Present one complete result:

- the final **Version/release plan** copied from fresh evidence, including
  anchors, semantic impacts, planned versions, dependency/group movements, and
  alignment-only packages marked as not published;
- the release date and exact package-to-changelog coverage;
- fresh compatibility and final-verification evidence, with incomplete
  comparisons or blockers called out;
- the committed branch and ready-for-review PR state.

Do not describe the result as ready while any package, changelog, plan effect, or
required verification is missing. The result is valid only for the recorded
history and target commits. Do not merge, enable auto-merge, tag, or publish.
