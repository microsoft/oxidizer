---
name: integration-test-policy
description: |
  Apply this repository's test policy when you add or change tests. Prefer
  public integration tests for public behavior and API contracts. Use unit
  tests only for implementation details that public APIs cannot exercise.
---

# Integration test policy

Use public integration tests under `crates/<crate>/tests/` for changes to
public behavior and API contracts.

Use unit tests only for implementation details that a supported public API
cannot exercise. State this limit in the test name or a nearby explanation.

Do not weaken, delete, skip, or rewrite an integration test to match an
implementation change.

If a change intentionally breaks behavior that an integration test covers,
describe the contract and test changes clearly in the pull request.

If code changes public behavior, add or update a public integration test. This
rule does not apply when the public API cannot reach the scenario.
