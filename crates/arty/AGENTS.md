# Arty agent guidance

## Panic handling

- Preserve the caller-facing contract in `docs/PANICS.md`. Keep implementation
  rules in this file rather than exposing them as public guarantees.
- Catch panics from code Arty explicitly runs for the user: async task polling,
  task factories, and blocking callbacks.
- A contained task or callback panic becomes `JoinError` without replacing
  the original panic.
- Let the executor apply the configured `PanicPolicy` to async task factories
  and polls. Forward its `ShutdownCause` without another panic-policy decision.
- For blocking callbacks, apply the policy at the blocking execution boundary.
  Publish the cause before delivering the panic result. Wake the associated
  async worker only when publication starts shutdown.
- Keep panic policy and worker wakers out of shared shutdown state.
- The shutdown policy starts coordinated runtime shutdown. The ignore policy
  leaves the scheduler usable.
- Add cleanup containment only when needed for memory safety or exact-once
  release of runtime-owned task storage. Keep the catch as narrow as possible.
- Do not add recovery guarantees for panicking destructors, custom wakers,
  notification hooks, poisoned state, or failed runtime invariants merely to
  make a test pass.
- Unsupported panics may abort, leak, or stop progress, but must not cause
  undefined behavior.
- Do not add dedicated recovery for failed runtime invariants, poisoned state,
  or unsafe-contract violations. An existing outer task boundary may still
  report such a panic through `JoinError`.
- Add `catch_unwind` only for a realistic safe-Rust path with a sound
  post-panic state and useful caller-visible behavior. If any of these are
  absent, let the panic propagate or abort naturally.
- Test public panic guarantees and catches required for storage safety. Do not
  add a `catch_unwind` only because a test can construct a panicking `Drop` or
  `Waker`.

## Generated documentation

- Do not edit `crates/arty/README.md` by hand. Regenerate it with
  `just anvil-readme --fix` after changing crate-level documentation.
