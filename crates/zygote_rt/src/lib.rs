// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(missing_docs)]

//! Integrates an executable with `zygote_control` for repeated launches.
//!
//! Add this crate to the executable that a `zygote_control::Zygote` launches.
//! The executable remains directly runnable. On Linux, the controller can
//! reuse an initialized template; on other platforms, the integration invokes
//! the application normally for each native process.
//!
//! # Choose an integration mode
//!
//! | Mode | Use it when |
//! |---|---|
//! | **Transparent** | The existing `main` should run unchanged and startup before `main` is inexpensive. |
//! | **Prepared** | Expensive immutable state can safely be constructed once and shared with every launched child. |
//!
//! Transparent mode keeps an existing `main` unchanged. Prepared mode lets the
//! target construct immutable state once per template and supplies that state
//! to each launched child.
//!
//! # Target setup
//!
//! Add the runtime as both a normal dependency and a build dependency:
//!
//! ```toml
//! [dependencies]
//! zygote_rt = "0.1"
//!
//! [build-dependencies]
//! zygote_rt = { version = "0.1", features = ["build"] }
//! ```
//!
//! Add a `build.rs` that configures the final executable:
//!
//! ```text
//! fn main() {
//!     zygote_rt::build::configure();
//! }
//! ```
//!
//! On Linux, the final linker must support GNU-style `--wrap`. The crate uses
//! checked-in Rust sources and does not require a C compiler or `protoc`.
//!
//! # Transparent mode
//!
//! A transparent target keeps an ordinary `main`. Direct execution and
//! controller launches invoke that same function with the selected arguments,
//! environment, working directory, and standard streams:
//!
//! ```no_run
//! # #![allow(clippy::needless_doctest_main)]
//! zygote_rt::link!();
//!
//! fn main() {
//!     println!("{:?}", std::env::args_os().collect::<Vec<_>>());
//! }
//! ```
//!
//! Place [`link!`] once at module scope. No other application changes are
//! required.
//!
//! # Prepared mode
//!
//! A prepared target constructs immutable state once per Linux template
//! worker. Direct execution still prepares once and runs the application
//! normally:
//!
//! ```no_run
//! use zygote_rt::{Launch, Prepared, ZygoteSafe};
//!
//! struct State {
//!     greeting: String,
//! }
//!
//! // This is sound because State contains immutable owned data and no threads,
//! // locks, process-specific handles, or pointers into external storage.
//! unsafe impl ZygoteSafe for State {}
//!
//! fn prepare() -> Result<Prepared<State>, &'static str> {
//!     Ok(Prepared::new(State {
//!         greeting: "hello".to_owned(),
//!     }))
//! }
//!
//! fn application(state: &'static State, launch: Launch<'_>) -> i32 {
//!     println!(
//!         "{} {:?}",
//!         state.greeting,
//!         launch.args_os().collect::<Vec<_>>()
//!     );
//!     0
//! }
//!
//! zygote_rt::prepared_main!(prepare, application);
//! ```
//!
//! The application callback receives the same prepared state for every launch
//! from a template and a [`Launch`] containing that launch's arguments.
//! [`Launch::args_os`] borrows those arguments for the callback; use
//! [`Launch::into_args_os`] when they must be retained.
//!
//! # Prepared-state safety
//!
//! Implementing [`ZygoteSafe`] is an explicit safety assertion. Prepared state
//! must not contain live threads, held locks, writable process-shared state,
//! child-specific secrets, or kernel resources whose duplication would couple
//! launches. Prefer immutable owned data such as parsed configuration,
//! lookup tables, and read-only model data.
//!
//! Preparation should finish before starting thread pools, asynchronous
//! runtimes, network clients, or telemetry exporters. Initialize those
//! child-specific resources inside the application callback instead.
//! Preparation failure exits without accepting launches.
//! On Linux, `Prepared::protected` can place the root value in a dedicated
//! mapping that becomes read-only before launches begin. This is not recursive:
//! heap allocations referenced by the root retain their original protection.
//!
//! # Launch behavior
//!
//! In transparent mode, use [`std::env::args_os`] as usual. In prepared mode,
//! use [`Launch::args_os`]. In both modes, `zygote_control` applies the
//! requested environment, working directory, standard streams, process
//! settings, and sandbox before application code runs.
//!
//! Each configured Linux worker prepares independently. Prepared data is
//! inherited through copy-on-write memory, so keeping it immutable preserves
//! sharing; writing to inherited pages increases each child's private memory.
//! The controller reports preparation, integration, and specialization
//! failures rather than silently launching without acceleration.
//! When the controller enables its prefork pool, dormant single-use workers
//! inherit this same prepared snapshot and receive launch-specific state only
//! after assignment.
//!
//! # Integration checklist
//!
//! 1. Add `zygote_rt` as both a normal and build dependency.
//! 2. Call `zygote_rt::build::configure()` from the target's `build.rs`.
//! 3. Choose exactly one entry mode: [`link!`] or [`prepared_main!`].
//! 4. Keep template initialization single-threaded.
//! 5. Launch the resulting executable through `zygote_control::Zygote`.
//!
//! Complete runnable targets, their manifest, and their build script are
//! maintained in the
//! [integration fixtures](https://github.com/microsoft/oxidizer/tree/main/crates/zygote_control/test-fixtures/targets).

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/zygote_rt/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/zygote_rt/favicon.ico")]

#[cfg(feature = "build")]
#[doc(hidden)]
pub mod build;

#[cfg(target_os = "linux")]
mod native;
mod prepared;
#[doc(hidden)]
pub mod protocol;

#[doc(hidden)]
#[cfg(target_os = "linux")]
pub use native::__wrap_main as __zygote_rt_wrap_main;
#[cfg(all(target_os = "linux", feature = "private-test-util"))]
#[doc(hidden)]
pub use native::{NativeDecodeObservation, NativeLaunchDecoder};
#[doc(inline)]
pub use prepared::{Launch, Prepared, ZygoteSafe, run_prepared};

/// Forces the pre-runtime wrapper into a transparent target's final link.
///
/// Place this once at module scope in transparent targets. It does not modify
/// or wrap `main`, so attribute-based main rewriters remain compatible.
#[macro_export]
macro_rules! link {
    () => {
        #[cfg(target_os = "linux")]
        #[used]
        static ZYGOTE_RT_LINK_MARKER: unsafe extern "C-unwind" fn(core::ffi::c_int, *mut *mut core::ffi::c_char) -> core::ffi::c_int =
            $crate::__zygote_rt_wrap_main;
    };
}

/// Defines a prepared zygote entry point.
///
/// The preparation function runs once and returns [`Prepared`] state. The
/// application function runs once per launch and receives that state plus the
/// launch arguments.
#[macro_export]
macro_rules! prepared_main {
    ($prepare:path, $application:path) => {
        #[cfg(target_os = "linux")]
        #[used]
        #[unsafe(link_section = "zygote_rt_mode")]
        static ZYGOTE_RT_PREPARED_MODE: u8 = 1;

        fn main() {
            let code = $crate::run_prepared($prepare, $application);
            if code != 0 {
                std::process::exit(code);
            }
        }
    };
}
