# Observed interest-query implementation

## Interest aggregation

The [design](../DESIGN.md#sink-interest) separates processor interest from event construction and delivery.
External event sources use `Sink::is_interested` before collecting fields; ordinary emission uses the same query before evaluating its event builder.

`Sink::is_interested` matches the sink representation. A single sink delegates to `SingleSinkState::is_interested`, which uses `any` over `EventProcessor::is_interested`.
A composite uses `any` over its flattened child states, and a no-op sink returns false.
Empty processor and child lists are uninterested. Composite construction keeps distinct leaf enrichment slots; interest queries do not access those slots.

The query has no event value and enters neither construction nor dispatch.
Clock reads, enrichment lookup, and event sampling belong to `SingleSinkState::dispatch`; processor-level filtering and log sampling belong to processor delivery.
Neither path is part of the interest query.

Interest is not cached. The processor contract permits the event description plus initialization state that changes at most once, and that transition can either enable or disable an event.
Emission repeats the same interest checks so an external query cannot reserve a delivery or bypass routing.

Public API tests use frozen clocks, independently constructed composite leaves, and `OnceLock` initialization in both directions.
Processor and sampler counters distinguish interest queries from delivery, flushing, and sampling.
