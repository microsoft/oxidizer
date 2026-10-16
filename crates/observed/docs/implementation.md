# Observed emission implementation

## Interest aggregation

The [design](../DESIGN.md#sink-interest) separates interest checks from event construction and delivery.
`Sink::is_interested` uses `any` over processors or flattened child sinks, without accessing event values, clocks, enrichments, or samplers.
Decisions are not cached across emissions.

Typed `Sink::emit` checks `E::DESCRIPTION` before invoking the builder and adapting the result through `EvaluatedEvent`.
Construction remains outside the reentrancy guard so field initializers can emit their own telemetry.

## Constructed dynamic events

`emit_dyn_event` passes its existing event reference to `Sink::emit_dynamic`.
Routing captures the description and selects the first interested recipient before acquiring one guard for the entire dispatch.
A borrowed closure retains that recipient and the remaining iterator, avoiding a second admission pass and any recipient-list allocation.
Earlier decisions are not revisited within this emission.

## Shared delivery

After initial admission, typed and sampled dynamic dispatch read the leaf clock and run any sampler before fresh recipient checks.
Dynamic dispatch without a sampler retains its first selected recipient and checks only the remaining processors.
Both paths use `SingleSinkState::deliver` to capture enrichment and share one timestamp and view across the leaf.
The recipient iterator stays lazy, so later interest checks observe preceding processor callbacks.
Keeping sampling outside this shared stage preserves fresh recipient checks after a sampler completes initialization.
