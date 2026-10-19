# Releasing Oxidizer Packages

Prepare releases on demand with the
[`increment-versions` skill](../.github/skills/increment-versions/SKILL.md).
Ordinary code PRs do not need version changes; the skill assesses accumulated
changes when you request a release.

## Prepare a release PR

1. Open a Copilot session on a new feature branch from updated `main`.
2. Ask Copilot:

   ```text
   Run /increment-versions to prepare a release PR for this workspace.
   Refresh origin/main and use it as release history. There is no unmerged parent.
   Do not merge or publish.
   ```

3. Let the agent check prerequisites, assess changes, update versions, and
   prepare the PR. It will ask for missing inputs or installation permission.
4. Review the PR's **Version/release plan**, including dependent-package changes
   and any unresolved compatibility checks. Require the normal repository checks
   before approving.
5. Merge under the normal review policy, then monitor the existing internal
   Microsoft publication automation and verify the expected versions on crates.io.

For a stacked release PR, identify its actual parent target in the request
instead of saying there is no unmerged parent. If code or history changes during
review, ask the agent to reassess before merging.

If Copilot cannot find the skill, start a new session or use `/skills reload`
in Copilot CLI.

## What stays unchanged

Merging a version PR still triggers the existing publication automation; the
skill itself does not publish or grant merge permission. No additional local
publish command is needed.

Existing changelogs remain historical records; the skill does not generate new
entries. Current release decisions are explained in the PR.
