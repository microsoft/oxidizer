---
name: release-changelogs
description: Write package CHANGELOG.md release notes from an already resolved cargo-release-plan version plan. Use after increment-versions applies the plan; does not choose versions or reconstruct missing plan data.
license: MIT
---

# Prepare release changelogs

Prepend one release entry to every package `CHANGELOG.md` named by the final
resolved version plan. Describe the complete consumer-visible change since each
package's anchor, including releases caused only by dependency updates or version
group alignment.

This skill consumes version-planning evidence. It never chooses versions or
semantic impacts.

## Required inputs

Obtain all of these from the caller or the completed
[`increment-versions`](../increment-versions/SKILL.md) run:

- the final resolved plan and report, after the plan was applied;
- the selected release-history and merge-target commit IDs;
- each planned package's anchor or explicit first-release state, planned
  version, and resolved dependency or group movements;
- one explicit ISO `YYYY-MM-DD` release date;
- the assessed checkout containing the applied version changes.

Stop when any input is absent, contradictory, stale, or cannot be tied to the
current checkout. Do not infer a version, anchor, date, package set, or dependency
movement from nearby manifests or historical changelog entries.

## Invariants

- Cover every package in the resolved plan, including non-publishable packages
  moved for version alignment.
- Use the exact planned version and supplied release date.
- Summarize all consumer-visible source, API, behavior, feature, platform,
  documentation, and requirement changes since that package's anchor. For an
  explicit first release with no anchor, describe the initial public surface
  without inventing a predecessor.
- Include exact local dependency version changes and explain dependency-only or
  group-only releases even when the package has no direct source change.
- Preserve the changelog preamble, local section style, links, and every
  historical release entry. Never rewrite released history.
- Keep statements evidence-based and package-specific. Do not copy commit titles
  that do not describe a consumer-visible effect.

## Procedure

1. Verify that the applied checkout and package/version set match the resolved
   plan. Locate each package through the final report, not by guessing directory
   names.
2. For each planned package, inspect its report entry and diff, compatibility
   evidence, source and documentation changes, resolved dependency effects, and
   relevant history from the package anchor through the assessed checkout. For
   a reported first release, assess the initial consumer-visible surface.
3. Draft a concise entry headed `## [VERSION] - YYYY-MM-DD`. Preserve the file's
   established organization where practical. If there is no direct
   consumer-visible change, add a maintenance entry naming the dependency or
   alignment reason instead of inventing a feature or fix.
4. Draft and validate the complete package set before writing. Insert each entry
   after the changelog title and preamble, before the newest historical release.
   If a planned version already exists in released history, stop and report the
   conflict.
5. During the coordinator's convergence loop, replace or remove only uncommitted
   entries created by this release run from its immediately preceding plan. Never
   prepend a second entry for the same run or alter older release history.
6. Verify the invariants and the exact one-to-one correspondence between the
   final plan and changelog entries.

## Failure and handoff

Report missing changelogs, ambiguous anchor state, stale evidence, duplicate
versions, unverifiable claims, or plan/checkout mismatches as blockers. Do not
write a partial package set. If a write fails, undo only this run's changelog
insertions and preserve unrelated or historical content.

Return the package-to-changelog coverage, the evidence used for each entry, and
all blockers. Changelog edits invalidate the earlier final evidence; the caller
must reassess the edited checkout.
