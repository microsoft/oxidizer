# Hedging Module Analysis

Analysis of `crates/seatbelt/src/hedging/` for correctness, performance, and usability.

## Correctness

### 1. ~~`last_result` lost when transitioning to drain phase in `run_delay_loop`~~ (Fixed)

**File:** `service.rs`  
**Severity:** High — could cause a panic at runtime

In `run_delay_loop`, when a recoverable result arrived and `launch_hedge` was called but
`clone_input` returned `None` (no clone available), no new future was pushed to `futs`. If
`hedges_launched` had reached `max_hedged`, the next loop iteration entered the `else`
branch and called `drain_for_first_acceptable(futs)`. At that point `futs` was **empty** and
`drain_for_first_acceptable` maintained its own local `last_result = None`, so it would panic
with `"at least one attempt was launched"`.

**Fix applied:** Inlined the drain loop in the `else` branch so it shares `last_result` with
the outer scope.

### 2. ~~Wildcard arm in `is_recoverable` hides new `RecoveryKind` variants~~ (Documented)

**File:** `service.rs`  
**Severity:** Low

The `_ => false` wildcard in `is_recoverable` is required because `RecoveryKind` is
`#[non_exhaustive]`, but means new variants silently become non-recoverable.

**Fix applied:** Added a comment documenting this intentional design choice.

---

## Performance

### 3. Double-boxing in immediate mode

**File:** `service.rs`  
**Severity:** Low — one extra allocation per hedge

`run_immediate` pushes `Box::pin(launch(cloned))` into `FuturesUnordered`. However,
`FuturesUnordered` internally heap-allocates each task in its own `Task<Fut>` wrapper.
This means each hedge future gets two heap allocations: one from `Box::pin` and one from
`FuturesUnordered::push`. For the delay/dynamic path, futures go directly into
`FuturesUnordered` (single allocation).

This is acceptable as an intentional trade-off to keep the async state machine size bounded.
The extra allocation per hedge in immediate mode (where N concurrent requests run) is
negligible compared to the I/O cost of the hedged operation itself.

### 4. ~~`on_hedge` and `emit_telemetry` called even when `clone_input` returns `None`~~ (Fixed)

**File:** `service.rs`

In both `run_immediate` and `launch_hedge`, `invoke_on_hedge` and `emit_telemetry` were called
**before** checking if `clone_input` succeeds. If the clone failed, the hedge was never actually
launched, but telemetry reported it as launched.

**Fix applied:** Moved `invoke_on_hedge` and `emit_telemetry` inside the `if let Some(cloned)`
block so they only fire when a hedge is actually launched.

### 5. `delay_for` called with `hedges_launched` before the hedge is launched

**File:** `service.rs`

```rust
let delay = self.hedging_mode.delay_for(hedges_launched);
```

This is correct — `hedges_launched` is the 0-based index of the *next* hedge to launch, so
`delay_for(0)` = delay before the first hedge. Just noting this for clarity since the naming
could be confusing.

---

## Usability

### 6. `HedgingDelayArgs` uses `hedge_index: u32` while all other args use `Attempt`

**File:** `args.rs`

`CloneArgs` and `OnHedgeArgs` both expose `attempt: Attempt`, but `HedgingDelayArgs` exposes
a raw `hedge_index: u32`. This is inconsistent. While the semantics are slightly different
(hedge_index is 0-based for hedges only, while Attempt tracks the overall attempt), converting
to `Attempt` would provide a more uniform API. Users could still derive the hedge number from
`attempt.index() - 1`.

### 7. Recovery callback naming can be confusing for hedging

**File:** `layer.rs`

The recovery callback uses `RecoveryInfo::never()` to mean "result is acceptable" and
`RecoveryInfo::retry()` to mean "result is not acceptable, keep hedging". These names were
designed for the retry context where "retry" makes sense. In hedging, the mental model is
different — "retry" really means "not acceptable, wait for other hedges". This is a
documentation opportunity rather than an API change, since consistency with retry is valuable.

### 8. No metric emitted for the original request (attempt 0)

**File:** `service.rs`

Telemetry is only emitted for hedged attempts (index 1+), not for the original request
(index 0). In contrast, the `on_hedge` callback is also only called for hedges, which is
correct since it means "a hedge was launched". But for metrics, it could be useful to emit
an event for the overall hedging operation (start/completion) to track total hedging
utilization.

### 9. ~~Module docs missing `handle_unavailable` in recovery section~~ (Fixed)

**File:** `layer.rs`

**Fix applied:** Added `RecoveryInfo::unavailable()` and its interaction with
`handle_unavailable` to the `recovery_with` doc comment.

---

## Test Coverage

### 10. ~~No test for `clone_input` returning `None`~~ (Fixed)

**Fix applied:** Added `clone_returning_none_skips_hedge` integration test exercising
`clone_input_with` returning `None` for hedge attempts. This also exercises the bug-fix
path from finding #1.

### 11. ~~No test for `handle_unavailable` end-to-end behavior~~ (Fixed)

**Fix applied:** Added `handle_unavailable_continues_hedging` integration test that verifies
`handle_unavailable(true)` causes hedging to continue when `RecoveryInfo::unavailable()` is
returned, ultimately producing a successful result from a hedge.

---

## Summary

| # | Category | Severity | Status | Description |
|---|----------|----------|--------|-------------|
| 1 | Correctness | **High** | **Fixed** | `last_result` lost when entering drain phase after failed clone |
| 2 | Correctness | Low | **Documented** | Wildcard arm in `is_recoverable` hides new variants |
| 3 | Performance | Low | Won't fix | Double-boxing in immediate mode (intentional trade-off) |
| 4 | Performance | Medium | **Fixed** | Telemetry emitted even when hedge clone fails |
| 5 | Performance | Info | N/A | `delay_for` index semantics |
| 6 | Usability | Medium | Open | `HedgingDelayArgs` uses raw `u32` instead of `Attempt` |
| 7 | Usability | Low | Open | Recovery naming confusing in hedging context |
| 8 | Usability | Low | Open | No metric for original request or overall hedging operation |
| 9 | Usability | Low | **Fixed** | `handle_unavailable` not mentioned in recovery docs |
| 10 | Testing | Medium | **Fixed** | No test for `clone_input` returning `None` |
| 11 | Testing | Medium | **Fixed** | No integration test for `handle_unavailable` behavior |
