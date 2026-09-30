// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Runs an asynchronous entry point on an Arty runtime.
///
/// Enable the `macros` feature to use this attribute. The function takes one owned
/// [`Builtins`](crate::runtime::Builtins) argument. Its return value becomes the synchronous
/// entry point's return value, subject to [`Runtime::run`](crate::runtime::Runtime::run)'s bounds.
/// The asynchronous body runs on a runtime worker, not on the calling thread.
///
/// ```
/// # #[cfg(feature = "macros")]
/// #[arty::main]
/// async fn main(cx: arty::runtime::Builtins) {
///     cx.scheduler().spawn(async |_| {}).await;
/// }
/// # #[cfg(not(feature = "macros"))] fn main() {}
/// ```
///
/// # Worker limit
///
/// `workers = N` selects **at most** `N` asynchronous workers, with one worker per selected
/// processor. Fewer available processors means fewer workers, not a construction error.
/// `N` must be a nonzero integer literal, optionally suffixed with `usize`, that fits in
/// the target's `usize`. Zero and nonliteral counts are rejected during macro expansion;
/// target-width overflow is rejected by the compiler.
///
/// ```
/// # #[cfg(feature = "macros")]
/// #[arty::main(workers = 4)]
/// async fn main(cx: arty::runtime::Builtins) {
///     assert_eq!(cx.scheduler().spawn(async |_| 42).await, 42);
/// }
/// # #[cfg(not(feature = "macros"))] fn main() {}
/// ```
///
/// Without a worker limit, [`ProcessorCount::auto`](crate::runtime::ProcessorCount::auto)
/// remains the default; it currently selects all available processors. This option does
/// not configure blocking-task pools. Configure those separately through
/// [`RuntimeBuilder::blocking_pool_policy`](crate::runtime::RuntimeBuilder::blocking_pool_policy).
///
/// # Custom builder
///
/// `builder = expression` accepts an expression producing a
/// [`RuntimeBuilder`](crate::runtime::RuntimeBuilder). The expression is evaluated exactly
/// once on the calling thread, before the runtime's workers start. Use it for processor
/// policies, computed settings, clocks, telemetry, or blocking-pool configuration.
/// Call a factory explicitly, as in `builder = app_builder()`.
///
/// ```
/// # #[cfg(feature = "macros")]
/// fn app_builder() -> arty::runtime::RuntimeBuilder {
///     use std::num::NonZero;
///
///     use arty::runtime::{BlockingPoolPolicy, ProcessorCount, Runtime};
///
///     Runtime::builder()
///         .processor_count(ProcessorCount::at_most(NonZero::new(4).unwrap()))
///         .blocking_pool_policy(BlockingPoolPolicy::shared(8))
/// }
///
/// # #[cfg(feature = "macros")]
/// #[arty::main(builder = app_builder())]
/// async fn main(cx: arty::runtime::Builtins) {
///     cx.scheduler().spawn(async |_| {}).await;
/// }
/// # #[cfg(not(feature = "macros"))] fn main() {}
/// ```
///
/// `builder` and `workers` cannot be combined. The builder expression cannot use the
/// injected `Builtins` argument or variables declared inside the asynchronous body.
/// It must produce a builder, not a runtime, a future, or a `Result`. An explicit
/// `builder = try_builder()?` works when the entry point's return type permits `?`,
/// including through a type alias; the macro never inserts `?` automatically.
///
/// Use `runtime_path = ::renamed_arty::runtime` when the crate is renamed or re-exported.
/// The override applies to every generated runtime reference, not to paths inside a
/// user-supplied builder expression.
///
/// # Panics
///
/// Panics if runtime construction fails or the root task panics. Limiting workers does
/// not prevent other startup failures, such as worker-thread creation failures.
/// Use [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) directly when
/// construction errors need to be returned instead.
pub use arty_macros::main;
/// Runs an asynchronous test on an Arty runtime.
///
/// Enable the `macros` feature to use this attribute. The function takes one owned
/// [`Builtins`](crate::runtime::Builtins) argument and may take an owned clock control as
/// its second argument. Standard test attributes, such as `#[should_panic]` and `#[ignore]`,
/// visibility, and the body's return value are preserved. Ignored tests do not evaluate
/// their builder or create a clock unless explicitly run.
///
/// ```
/// # #[cfg(feature = "macros")]
/// #[arty::test]
/// async fn answer(cx: arty::runtime::Builtins) {
///     assert_eq!(cx.scheduler().spawn(async |_| 42).await, 42);
/// }
/// ```
///
/// The [`main`] attribute's `workers`, `builder`, and `runtime_path` options also apply
/// to tests. Each invocation gets a separate runtime; omitting `workers` preserves the
/// runtime's automatic processor policy rather than selecting a single worker.
/// The builder is evaluated on the calling test-harness thread, not a runtime worker.
///
/// # Simulated time
///
/// Enable Arty's `test-util` feature in a **dev-dependency** and add a second
/// `arty::time::ClockControl` parameter to opt into simulated time. Both parameter types
/// may use aliases, including qualified associated types; parameter names are arbitrary.
/// The control must be owned, not borrowed.
///
/// The macro creates one fresh control before building the runtime, installs a clone
/// through [`RuntimeBuilder::clock`](crate::runtime::RuntimeBuilder::clock), and moves
/// the original handle into the test body. All those handles control the same time
/// domain. Worker clocks are activated by the runtime during startup.
/// The initial wall-clock time is the UNIX epoch and automatic advancement is disabled.
/// Enabling `test-util` alone does not change one-argument tests.
///
/// ```
/// # #[cfg(all(feature = "macros", feature = "test-util"))]
/// #[arty::test(workers = 1)]
/// async fn simulated_delay(cx: arty::runtime::Builtins, control: arty::time::ClockControl) {
///     use std::time::Duration;
///
///     let control = control.auto_advance_timers(true);
///     let watch = cx.clock().stopwatch();
///     cx.clock().delay(Duration::from_secs(30)).await;
///     assert_eq!(watch.elapsed(), Duration::from_secs(30));
///
///     control.advance(Duration::from_secs(5));
///     assert_eq!(watch.elapsed(), Duration::from_secs(35));
/// }
/// ```
///
/// Use `control.advance(duration)` to move time manually; ensure a pending timer has
/// been polled before advancing it. `auto_advance(duration)` advances on time reads.
/// `auto_advance_timers(true)` fires timers eagerly as they are registered or time is
/// read: it is not an idle-runtime policy and does not simulate concurrent timers
/// completing in deadline order. Use manual advancement when that ordering matters.
///
/// A clock-control parameter can be combined with `workers`, but **not** with `builder`.
/// This prevents silently replacing a clock already supplied by a custom builder.
/// To combine a custom builder with controlled time, use an ordinary synchronous test,
/// create a control, and explicitly call `builder.clock(control.clone()).build()` before
/// running the asynchronous body.
///
/// Use `runtime_path = ::renamed_arty::runtime` when the crate is renamed or re-exported.
///
/// # Panics
///
/// Panics if runtime construction fails or the test body panics. The original task
/// panic payload is preserved for `#[should_panic(expected = "...")]`.
pub use arty_macros::test;
