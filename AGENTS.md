# AI Agents Guidelines

Code in this repository should follow the guidelines specified in the [Microsoft Rust Guidelines](https://microsoft.github.io/rust-guidelines/agents/all.txt).

## README Files

Crate README files are auto-generated via `just readme`. Do not manually update them.

## Executing `just` commands

If you only touch one crate, you may use `just package=crate_name command` to narrow command scope to one crate.

## Pre-commit Checklist

- Run `just format` to format code.
- Run `just readme` to regenerate crate-level readme files.
- Run `just spellcheck` to check spelling in code comments and docs.

## Spelling

The spell checker dictionary is in the `.spelling` file, one word per line in arbitrary order.

## Changelogs

The changelogs are updated by `scripts/release-crate.ps1` at release time, based on Git history. It is not necessary to make manual edits
to the changelogs, though you are permitted to do so if explicitly instructed.

## Releasing Packages

See [docs/releasing.md](docs/releasing.md) for the release tooling
reference: glossary (direct/transitive dependent vs dependency, cascade
direction, change type vs version component, release set, pending
release, elevation), the cascade-organisation invariants, and the
workflow for `scripts/release-crate.ps1` and
`scripts/check-unreleased-dependencies.ps1`.

## Pull Requests

Pull request titles must follow [Conventional Commits](https://www.conventionalcommits.org/) naming, e.g. `feat(bytesbuf): add new metric` or `fix(cachet): correct eviction logic`.

## Feature-gated Doctests

Doctests that reference items behind a Cargo feature must compile both with and without that feature; wrap their bodies in hidden `#[cfg(...)]` shims. See [AGENTS-feature-gated-doctests.md](AGENTS-feature-gated-doctests.md).

## Required CI Checks

The `required-checks` job in `.github/workflows/main.yml` is a "fan-in"
aggregator: branch protection requires only this single context for jobs
defined in that workflow, and it succeeds when every dependency either
succeeded or was skipped.

When you add a new job to `main.yml`, you MUST also add it to the `needs:`
list of `required-checks` if it has BOTH a `strategy.matrix` AND a
job-level `if:` that can evaluate to false (typically gated on
`needs.delta.outputs.skip` or `github.event_name`). GitHub Actions does
not expand the matrix when such a gate skips the job, so per-OS contexts
like `testing (ubuntu-latest)` are never posted and would stay stuck on
`Expected — Waiting for status to be reported` if required directly.

Other required jobs should also be funnelled through `required-checks`
so branch protection only references one workflow context. See the
inline comment on the `required-checks` job for the full policy.

## Maintainability

While it is fine to use `.expect()`, the precondition is that it is either a programming error (the caller did something wrong)
or a situation that can never happen (in the absence of bugs). The expect-message must document either what the caller did wrong
in their code or why we believe the situation could never happen.

This is bad code: `self_span.get(self_offset..).expect("self_offset out of bounds")` - it does not explain what the caller did
wrong and it does not explain why we believe this access can never be out of bounds.

This is good code: `self_span.get(self_offset..).expect("guarded by min() above to never exceed span length")` - this explains
why we believe the operation can never cause an out of bounds access.
