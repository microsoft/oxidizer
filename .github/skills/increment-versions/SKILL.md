---
name: increment-versions
description: Prepare an on-demand version-increment PR using cargo-release-plan. Use only when explicitly asked to prepare package versions or a release plan; Oxidizer uses standalone mode and keeps publication separate.
license: MIT
---

# On-demand version preparation

Use the upstream `increment-versions` skill in **standalone mode**, only when
explicitly asked to prepare package versions. Do not run it for ordinary code
PRs. Read [repository release policy](../../../docs/releasing.md) and follow
[AGENTS.md](../../../AGENTS.md).

## Load the pinned upstream skill

Before planning or changing release inputs, fetch and read all four complete
resources from `folo-rs/folo` commit
`fcc52639a902c8d8ad8000a58e0dfc39637bb6ac`:

- [SKILL.md](https://raw.githubusercontent.com/folo-rs/folo/fcc52639a902c8d8ad8000a58e0dfc39637bb6ac/.github/skills/increment-versions/SKILL.md)
- [commands.md](https://raw.githubusercontent.com/folo-rs/folo/fcc52639a902c8d8ad8000a58e0dfc39637bb6ac/.github/skills/increment-versions/commands.md)
- [determining-level.md](https://raw.githubusercontent.com/folo-rs/folo/fcc52639a902c8d8ad8000a58e0dfc39637bb6ac/.github/skills/increment-versions/determining-level.md)
- [LICENSE](https://raw.githubusercontent.com/folo-rs/folo/fcc52639a902c8d8ad8000a58e0dfc39637bb6ac/.github/skills/increment-versions/LICENSE)

Follow the upstream procedure, command reference, and decision guide together.
Resolve their relative links against that same pinned directory, not this local
wrapper. Retrieve any truncated content before proceeding. If retaining local
copies, keep the complete directory, including its license, outside the
repository or in an already ignored location.

Network access is required. If a resource is unavailable, incomplete, or
restricted, stop and report the blocker. Do not substitute remembered
instructions, an abbreviated procedure, another revision, or a link without
reading its content.

## Repository boundaries

Require the explicitly selected workspace and local release-history ref, plus
the actual parent target ref for a stacked PR. Ask for missing inputs rather
than inferring them from publishing configuration.

Check the installed tool's schemas using the loaded upstream instructions.
On a mismatch, stop and report the incompatible schemas. Updating this wrapper's
pinned revision and the documented schema expectations is a separate reviewed
change; do not silently follow upstream `main` or replace the wrapper.
Install or upgrade tools only with the applicable permission.

Use only the upstream standalone path. Do not add publication configuration,
registry/publication gates, Trusted Publisher setup, or publishing workflows.
Preserve existing publication automation: merging a version PR can still trigger
it. This skill grants neither merge nor publication authority.

Include the upstream procedure's complete **Version/release plan** in the PR,
including dependent movements, alignment-only packages, retained increments,
and incomplete comparisons. Keep all assessment evidence out of commits.
