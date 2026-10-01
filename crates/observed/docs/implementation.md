# Observed interest-query implementation

## Interest aggregation

The [design](../DESIGN.md#sink-interest) separates processor interest from event construction and delivery.
External event sources use `Sink::is_interested` before collecting fields; ordinary emission uses the same query before evaluating its event builder.

`Sink::is_interested` matches the sink representation. A single sink delegates to `SingleSinkState::is_interested`, which uses `any` over `EventProcessor::is_interested`.
A composite uses `any` over its flattened child states, and a no-op sink returns false.
Empty processor and child lists are uninterested. Composite construction keeps distinct leaf enrichment slots; interest queries do not access those slots.

The query has no event value and enters neither construction nor dispatch.
Clock reads, enrichment lookup, and event sampling belong to leaf dispatch; processor-level filtering and log sampling belong to processor delivery.
Neither path is part of the interest query.

Interest is not cached across emissions. The processor contract permits the event description plus initialization state that changes at most once, and that transition can either enable or disable an event.
Checks are independent observations, not an atomic snapshot or a requirement to reconsider earlier decisions during the same emission.
Emission repeats the same interest checks so an external query cannot reserve a delivery or bypass routing.

Public API tests use frozen clocks, independently constructed composite leaves, and `OnceLock` initialization in both directions.
Processor and sampler counters distinguish interest queries from delivery, flushing, and sampling.

## Constructed dynamic events

`emit_dyn_event` passes its existing event reference to `Sink::emit_dynamic`, without wrapping it in the lazy typed construction state.
Routing captures one event description and selects the first interested recipient before acquiring the reentrancy guard.
One-time initialization during this admission check can emit telemetry. A wholly uninterested event returns without taking the guard.
The selected recipient and remaining iterator are carried in a borrowed dispatch closure, without an allocation.
The guard covers that dispatch and all subsequent recipients and leaves, including their interest checks and samplers.
Typed emission retains its construction gate and evaluates its builder before acquiring the guard, so nested telemetry from field initializers remains supported.

A leaf without an event sampler selects its first interested processor before reading its clock or enrichment slot.
That processor and subsequent interested processors share one timestamp and enrichment view.
The remaining interest checks are evaluated in order after preceding processors run, without allocating a recipient list.
Recipients already checked are not reconsidered in that emission; subsequent emissions check interest afresh.
This single pass avoids a separate aggregate construction check for an event that already exists.

A sampled leaf uses its first interested recipient as an admission gate and delegates to the normal sampled dispatch.
After sampling, every recipient is checked again, including any processor rejected before sampling.
The sampler may complete processor initialization, so sampled dispatch selects recipients after that callback instead of retaining the admission selection.
