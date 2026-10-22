# Tick API decisions

## Normalized system-time conversion is intentionally infallible

`Clock::system_time_as` and `SimpleClock::system_time_as` return the requested
target value directly. They must not return `Result`, and they must not be
renamed to `try_system_time_as`.

This is intentional. Production clock data is normalized, and every format that
`tick` supports through this convenience API must represent that data. Callers
must not need an error branch for the supported conversion path.

The generic target still uses `TryFrom<SystemTime>` because ecosystem types use
that standard conversion trait. The methods panic if a caller selects a type
with an unsupported range, or if a controlled test moves time outside the
target type's documented range. Code that expects conversion failure must call
`Clock::system_time` and invoke the target type's `TryFrom` implementation
directly.

When a new `tick::fmt` type is added, its public integration tests must show
that `system_time_as` accepts representative normalized production clock data.

## Periodic timers do not expose `futures_core::Stream`

`PeriodicTimer::next` is the public iteration API. It returns a `TimerTick` with
the clock instant observed when the tick completed.

`PeriodicTimer` must not implement `futures_core::Stream`. A 0.x futures crate
in the public surface would couple every `tick` consumer to that external API.
Applications that need stream combinators can adapt the timer locally with
`futures::stream::unfold`, as shown in the `PeriodicTimer` rustdoc.

Keep `futures` as a development dependency only when examples or tests need its
executor or extension helpers. It must not be a normal dependency of `tick`.
