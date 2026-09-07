# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial release of Event sampling API through `EventSampler` and
  `Sink::with_event_sampler`. Each interested Sink calls its sampler once with
  a read-only `EventSamplingContext` which returns either
  `EventSamplingDecision::Continue` to continue normal event processing or
  `EventSamplingDecision::Drop` to discard the whole event for that Sink.

## [0.25.0] - 2026-08-27

- ⚠️ Breaking

  - Now requires `0.5.0` of `ohno`
  - Now requires `0.11.0` of `thread_aware`
  - Now requires `0.6.0` of `tick`

- ✨ Features

  - introduce the observed_utils crate ([#677](https://github.com/microsoft/oxidizer/pull/677))

- ✔️ Tasks

  - bump all_the_time to 0.6.2 ([#690](https://github.com/microsoft/oxidizer/pull/690))

## [0.24.0] - 2026-08-17

### Added

- Initial release of `observed`, a structured telemetry framework with typed
  events, enrichment, redaction, and per-field routing to OpenTelemetry.
- `#[event(...)]` and the `emit!` macro for defining and emitting typed
  telemetry events.
- Scoped, stackable enrichment via `#[derive(Enrichment)]` and RAII guards, with
  cross-thread context propagation.
- Data-classification-aware redaction integrated with the `data_privacy` crate.
- `Value::U64` and `From` conversions for the remaining integer widths (`i8`,
  `i16`, `u8`, `u16`, `u64`, `usize`, `isize`), so an unredacted numeric field
  keeps its own type instead of being widened, saturated or stringified. `u64`
  gets its own variant because more than half its range does not fit in `i64`,
  and byte and request counters live exactly in that half. Exporters need a
  `U64` arm; `Value` is `#[non_exhaustive]`, so adding the variant is
  source-compatible for matchers outside the crate. `u128` and `i128` remain
  unsupported: no telemetry backend represents them.

### Changed

- A metric instrument's value field must now be `#[unredacted]` and a supported
  numeric primitive, checked by `#[event(...)]` at compile time. Previously a
  classified value field, a non-numeric `gauge`/`histogram` field, or a width
  `Value` could not carry was accepted and then silently recorded nothing (or
  failed deep inside the expansion with `Value: From<u64> is not satisfied`).
- The reentrancy guard is now acquired after the event value has been built, so
  telemetry emitted by a helper called from a field initializer is no longer
  dropped. The guard covers processor dispatch only.
- The guard returned by `Transfer::apply_current_thread` is now `!Send`, so it
  cannot be dropped on a thread other than the one that applied it.

### Fixed

- `Value::from_redacted` no longer exports a partial string when a
  `RedactedDisplay` implementation fails part-way through; the value is erased
  instead. The re-entrant path no longer uses `to_string`, which panics on such
  a failure.
- Fields written with raw identifiers are exported under their plain name, so
  `r#type` is recorded as `type` rather than `r#type`.
