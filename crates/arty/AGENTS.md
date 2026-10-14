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

## Panic handling

- Follow the contract in `docs/PANICS.md`.
- Catch panics from code Arty explicitly runs for the user: async task polling,
  task factories, and blocking callbacks.
- Add cleanup containment only when needed for memory safety or exact-once
  release of runtime-owned task storage. Keep the catch as narrow as possible.
- Do not add recovery guarantees for panicking destructors, custom wakers,
  notification hooks, telemetry processors, poisoned state, or failed runtime
  invariants merely to make a test pass.
- Treat no-panic requirements as supported-behavior contracts, not unsafe
  preconditions. Unsupported panics may abort, leak, or stop progress, but must
  not cause undefined behavior.
- Before adding `catch_unwind`, identify a realistic safe-Rust path, the sound
  state that remains after the panic, and the useful behavior visible to the
  caller.

## Generated documentation

- Do not edit `crates/arty/README.md` by hand. Regenerate it with
  `just anvil-readme --fix` after changing crate-level documentation.
