# Releasing Oxidizer Packages

Ask Copilot to run the
[`prepare-release` skill](../.github/skills/prepare-release/SKILL.md) to prepare
an on-demand version PR:

```text
Run /prepare-release for this workspace.
Refresh origin/main and use it as release history, with no unmerged parent.
Use YYYY-MM-DD as the required release date.
Prepare a ready-for-review PR; do not merge or publish.
```

For a stacked PR, name the actual parent target.

## Prerequisites

Follow the tool and schema prerequisites owned by
[`increment-versions`](../.github/skills/increment-versions/SKILL.md). Keep its
generated evidence outside the repository or in an ignored directory. On
Windows, choose a short, session-unique evidence root such as
`C:\rp\<session-id>` to avoid Git path length failures and concurrent-run
collisions during preview.

## Comparison exceptions

If a published predecessor has no library but the current package adds one,
library API comparison has no matching predecessor. Use a reviewed
`--exclude-comparison PACKAGE@VERSION=REASON` exception only with evidence for
that exact baseline. Keep the exception in every compatibility stage and in the
release decision ledger. Assess the new library API and existing command-line,
behavior, and feature contracts. The exception does not mean that comparison
passed or that a patch release is sufficient.

The updated workflow requires compatibility schema 4. For an unpublished tool
fix, use a session-local executable and retain its checksum and source provenance.
Do not use its package version as evidence that the published release supports
the fix. Hosted use needs a matching published tool and skill.

## Boundaries

The workflow does not merge, tag, publish, or configure publishing. Merging an
approved version PR leaves publication to the existing Microsoft automation.
If release history, the target, or source changes during review, reassess before
merging.
