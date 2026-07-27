# GitHub Copilot instructions

## Copilot code review

The following instructions apply **only to Copilot code review** when it comments on
pull requests. They do not apply to the coding agent (see `AGENTS.md` for that).

- **Never assume or assert whether code compiles, builds, type-checks, or passes
  tests.** Every pull request is validated by CI (build, clippy, tests, and the
  other checks in `.github/workflows`). Do not leave review comments that claim
  code "won't compile", "fails to build", "has a type error", "is missing an
  import", "won't link", or otherwise predict a compiler, linter, or test
  outcome — CI is the source of truth for those. If you believe there is a real
  logic, correctness, security, or design problem, describe that concern
  directly without speculating about build or test results.
