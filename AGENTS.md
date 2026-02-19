# AI Agents Guidelines

Code in this repository should follow the guidelines specified in the [Microsoft Rust Guidelines](https://microsoft.github.io/rust-guidelines/agents/all.txt).

## README Files

Crate README files are auto-generated via `just readme`. Do not manually update them.

## Pre-commit Checklist

- Run `just format` before committing changes.
- Run `just readme` before pushing changes to the cloud to ensure crate README files are up to date.
