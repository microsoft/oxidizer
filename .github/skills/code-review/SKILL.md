---
name: code-review
description: Review guidance for pull requests in this repository. Use when reviewing a pull request, to keep comments focused on logic, correctness, security, and design instead of predicting build, lint, or test outcomes.
license: MIT
---

# Code review

This skill applies when reviewing pull requests. It does not apply to the coding
agent when it writes code — see `AGENTS.md` for that.

## Never predict build or test outcomes

**Never assume or assert whether code compiles, builds, type-checks, or passes
tests.** Every pull request is validated by CI (build, clippy, tests, and the
other checks in `.github/workflows`). Do not leave review comments that claim
code "won't compile", "fails to build", "has a type error", "is missing an
import", "won't link", or otherwise predict a compiler, linter, or test
outcome — CI is the source of truth for those.

If you believe there is a real logic, correctness, security, or design problem,
describe that concern directly without speculating about build or test results.
