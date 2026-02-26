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

Do not manually edit `CHANGELOG.md` files. Changelogs are automatically updated on release.

## Maintainability

While it is fine to use `.expect()`, the precondition is that it is either a programming error (the caller did something wrong)
or a situation that can never happen (in the absence of bugs). The expect-message must document either what the caller did wrong
in their code or why we believe the situation could never happen.

This is bad code: `self_span.get(self_offset..).expect("self_offset out of bounds")` - it does not explain what the caller did
wrong and it does not explain why we believe this access can never be out of bounds.

This is good code: `self_span.get(self_offset..).expect("guarded by min() above to never exceed span length")` - this explains
why we believe the operation can never cause an out of bounds access.
