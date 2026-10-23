// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// We assert unwind safety here because the topic is too much hassle to worry about and since
// #[should_panic] does not require us to worry about it, we are not going to worry about it here.
#[macro_export]
macro_rules! assert_panic {
    ($stmt:stmt$(,)?) => {
        #[allow(clippy::multi_assignments, reason = "macro untidiness")]
        #[expect(clippy::allow_attributes, reason = "macro untidiness")]
        ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| -> () { _ = { $stmt } }))
            .expect_err("assert_panic! argument did not panic")
    };
}

/// Installs the `#[ctor::ctor]` process-initialization function that initializes
/// `tracing` for a test binary.
///
/// `tracing` subscribers and the `tracing-core` callsite-interest cache are
/// process-global. Every unit-test and integration-test executable in a package
/// with a direct normal or dev-dependency on `tracing` must invoke this macro.
/// A target that explicitly tests tracing through a dependency must also invoke it.
/// Invoke it at module scope so a silent, always-interested subscriber is installed
/// before any test runs. Without it, `tracing` emission lines may be reported as lacking
/// test coverage even though they execute, and log capture may silently observe nothing.
/// The subscriber evaluates field expressions and formats events and span fields
/// synchronously even with no output enabled, including during mutation testing.
/// Apply the rule across declared features and platforms. Build-only dependencies,
/// procedural-macro host dependencies, and transitive presence alone do not qualify
/// a consumer. See `docs/tracing-tests.md` for the audit procedure and isolated
/// exceptions for disabled tracing and custom subscribers.
///
/// The invoking crate must have a dev-dependency on the [`ctor`](https://docs.rs/ctor)
/// crate, which supplies the expanded `#[ctor::ctor]` attribute.
///
/// Invoke it bare in an integration-test file (`tests/*.rs`). In a crate root (`lib.rs`),
/// gate it with `#[cfg(test)]` so it only affects the unit-test binary:
///
/// ```ignore
/// #[cfg(test)]
/// testing_aids::init_tracing!();
/// ```
#[macro_export]
macro_rules! init_tracing {
    () => {
        // The process-initialization function lives inside an anonymous `const` block so
        // its item name never collides, even if the macro is invoked more than once in the
        // same module.
        const _: () = {
            #[::ctor::ctor(unsafe)]
            fn init_tracing() {
                $crate::tracing_logs::initialize();
            }
        };
    };
}
