# Tick API decisions

## System-time conversion stays infallible

`Clock::system_time_as` and `SimpleClock::system_time_as` return the target
value. They must not return `Result`. Do not rename them to
`try_system_time_as`.

Production clock data is normalized. Each format that `tick` supports through
this convenience API must represent that data. Callers do not need an error
branch for a supported conversion.

The target type still implements `TryFrom<SystemTime>`. Other Rust types use
this standard conversion trait.

The methods panic in either of these cases:

- The caller selects a type with an unsupported range.
- A controlled test moves time outside the documented range of the target type.

If code expects conversion to fail, call `Clock::system_time`. Then call the
target type's `TryFrom` implementation directly.

If you add a `tick::fmt` type, add public integration tests. The tests must show
that `system_time_as` accepts representative normalized production clock data.

## Periodic timers do not implement `futures_core::Stream`

`Timer::next` is the public iteration API. It returns a `TimerTick`.
The tick contains the clock instant observed when the timer completed.

`Timer` must not implement `futures_core::Stream`. A public dependency
on a 0.x futures crate couples each `tick` consumer to that API.

Applications can use `futures::stream::unfold` to adapt a timer locally. The
`Timer` rustdoc shows this adapter.

Keep `futures` as a development dependency when examples or tests need it. It
must not be a normal dependency of `tick`.
