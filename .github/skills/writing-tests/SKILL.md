---
name: writing-tests
description: |
  Apply this repository's test policy when you add or change tests. Test public
  behavior through public APIs and test utilities. Keep tests deterministic,
  fast, and independent of real I/O. Use unit tests for implementation details
  that public APIs cannot exercise.
---

# Writing tests

This skill is the repository guide for developer agents that write tests.
Review agents use the stricter rules in
`.github/skills/code-review/test-review.md`.

## Test behavior through public APIs

Test public behavior and API contracts through the public API. Put these tests
under `crates/<crate>/tests/`.

Use test utilities to control collaborators and state. A public integration
test does not need real network, process, filesystem, or clock I/O.

Use unit tests for implementation details that a supported public API cannot
exercise. Keep these tests close to the implementation.

## Keep tests deterministic and fast

Control all inputs and dependencies. Do not depend on test order, host state,
wall-clock time, random values, external services, or developer credentials.

Prefer these workspace tools:

- Use `fakeable` to create controlled fakes and mocks.
- Use `future_assert` to test future behavior without starting a runtime.
- Use a frozen `tick` clock and advance time explicitly.
- Use `insta` snapshots for exact string and structured output.

Target less than one second for each test, excluding compilation. Prefer
in-memory values and injected collaborators.

## Preserve behavior tests during implementation changes

If a refactor or performance change preserves public behavior, do not rewrite
existing integration tests to match internal implementation details. Add tests
when the change needs more coverage.

If a change intentionally alters public behavior, update the affected tests.
Describe the contract and test changes clearly in the pull request.

Do not weaken assertions, skip tests, or delete coverage to make a change pass.
