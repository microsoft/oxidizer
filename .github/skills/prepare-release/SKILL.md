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

On Windows, use a very short external evidence root such as `C:\rp\<run>` and
new short child directories for every repeated stage. Deep session artifact
paths can exceed Git's path limit when preview materializes long snapshot names.
If Git still reports a long-path failure, retry the unchanged stage with
process-scoped `core.longpaths=true`; do not change persistent Git configuration.

## Workflow

1. Follow `increment-versions` with the supplied standalone inputs through
   preparation, compatibility analysis, and a resolved preview.
2. Before application, freeze a sorted **decision ledger** with one row per
   publishable package: anchor, chosen impact or no-release result, planned
   version, compatibility minimum, direct consumer-visible evidence, and
   dependency/re-export/group effect. Every impact above the compatibility
   minimum needs concrete cited evidence. When evidence categories imply
   different impacts, use the strongest impact they directly establish; when
   the evidence cannot establish one choice, stop for a human decision instead
   of choosing nondeterministically. Regenerate the preview if the audit changes
   a decision.
3. Apply the audited resolved plan. Retain its reports, plan, decisions,
   compatibility evidence, decision ledger, and **Version/release plan**
   handoff. Stop before `increment-versions`' terminal commit, PR, or merge
   actions; this coordinator owns the remaining handoff and never delegates
   merge.
4. Run `release-changelogs` against that exact resolved plan and release date.
5. Because changelog edits changed the assessed checkout, return to
   `increment-versions` and rerun its evidence and verification stages end to end
   in new evidence directories. Re-resolve the original refs first. If either
   moved, follow the component skill's recovery procedure and restart; preserve
   changelog edits and unrelated work.
6. Compare the fresh decisions and resolved plan to the decision ledger. Any
   changed semantic impact without changed source or compatibility evidence is a
   reproducibility failure: stop and report both evidence-backed alternatives.
   Do not silently accept whichever assessment ran last.
7. If the fresh resolved package/version set differs for an evidence-backed
   reason, apply again only after verifying that the pending delta is exactly the
   new plan. Then rerun `release-changelogs` and repeat steps 5-6. Stop only when
   the decision ledger, resolved plan, applied manifests and lockfile, and
   changelog entries agree.
8. Stop uncommitted on any changelog blocker, partial application, failed check,
   or evidence mismatch. Otherwise run the smallest relevant documentation and
   repository checks, commit only the intended version, requirement, lockfile,
   and changelog changes, and create or update the ready-for-review PR when
   requested. Never commit generated evidence or unrelated edits.

## Handoff

Present one complete result:

- the final **Version/release plan** copied from fresh evidence, including
  anchors, semantic impacts, planned versions, dependency/group movements, and
  alignment-only packages marked as not published;
- the final decision ledger and any reproducibility conflict resolved or left
  for human decision;
- the release date and exact package-to-changelog coverage;
- fresh compatibility and final-verification evidence, with incomplete
  comparisons or blockers called out;
- the committed branch and ready-for-review PR state.

Do not describe the result as ready while any package, changelog, plan effect, or
required verification is missing. The result is valid only for the recorded
history and target commits. Do not merge, enable auto-merge, tag, or publish.
