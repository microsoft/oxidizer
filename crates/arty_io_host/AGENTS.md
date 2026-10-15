# arty_io_host agent guidance

## Behavioral changes and tests

- Prefer public integration tests under `crates/arty_io_host/tests/` for context
  registration, role assignment, worker-local driver creation, interruption,
  cycle ordering, failures, diagnostics, and Drop-based teardown.
- Use unit tests only for private invariants that cannot be meaningfully
  exercised through the approved public API.
- Do not weaken, skip, or rewrite integration tests to match an implementation
  change.

## Crate positioning

- This is unstable implementation infrastructure for Arty, not a direct-use
  application or library API.
- Direct normal users to `arty`.
- Do not add standalone usage examples or compatibility promises.

## Generated documentation

- Do not edit `crates/arty_io_host/README.md` by hand. Regenerate it with
  `just anvil-readme --fix`.
