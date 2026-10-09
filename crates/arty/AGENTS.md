# Arty agent guidance

## Behavioral changes and tests

- Prefer public integration tests under `crates/arty/tests/` for changes to
  runtime behavior, lifecycle, scheduling, joins, shutdown, telemetry, panic
  handling, feature behavior, or public API contracts.
- Use unit tests for implementation details only when the scenario cannot be
  exercised through a supported public API. Keep that limitation clear in the
  test name or nearby explanation.
- Do not weaken, delete, skip, or rewrite an integration test to match an
  implementation change.
- Any breaking change to behavior asserted by an existing integration test
  requires explicit human approval before changing the test or contract.
- A code change that alters a public behavior must include or update a public
  integration test unless the scenario is genuinely unreachable through the
  public API.

## Generated documentation

- Do not edit `crates/arty/README.md` by hand. Regenerate it with
  `just anvil-readme --fix` after changing crate-level documentation.
