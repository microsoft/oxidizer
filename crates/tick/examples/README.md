# Time Examples

These examples demonstrate the features and capabilities of `tick` primitives:

- [Basic](basic.rs): Quick introduction to time primitives.
- [Basic Tokio](basic_tokio.rs): Basic usage of `Clock` with the Tokio runtime.
- [Clock](clock.rs): More examples of how to use the `Clock` type, `Stopwatch`, and `Timer`.
- [Timestamp](timestamp.rs): Working with `Timestamp` for UTC timestamps, including parsing and formatting of standard UTC timestamp formats.
- [Data](data.rs): Demonstrates how to integrate `Timestamp` with serializable data.
- [Interop Jiff](interop_jiff.rs): Showcases interoperability with the [jiff](https://docs.rs/jiff) crate, including formatting timestamps into time-zone-aware date-times.
- [Interop Chrono](interop_chrono.rs): Showcases interoperability with the [chrono](https://docs.rs/chrono) crate, including formatting timestamps into time-zone-aware date-times.
- [Interop Time](interop_time.rs): Showcases interoperability with the [time](https://docs.rs/time) crate, including formatting timestamps into time-zone-aware date-times.
- [Clock Control](clock_control.rs): Demonstrates how to use `ClockControl` to control the flow of time.
