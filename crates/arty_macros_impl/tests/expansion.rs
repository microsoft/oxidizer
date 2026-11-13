// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests behavior through the public API.

use arty_macros_impl::{main, test};
use insta::assert_snapshot;
use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::quote;
use testing_aids::render_expansion;

#[test]
fn main_preserves_visibility_and_return_type() {
    let expansion = main(
        TokenStream::new(),
        quote! {
            pub async fn main(cx: arty::task::Context) -> Result<(), Error> {
                run(cx).await
            }
        },
    );
    assert_snapshot!(render_expansion(&expansion), @r#"
    pub fn main() -> Result<(), Error> {
        let __arty_runtime = ::arty::runtime::Runtime::new()
            .expect("failed to create the runtime for the entry point");
        let __arty_result = __arty_runtime
            .scheduler()
            .block_on(async move |cx: arty::task::Context| { run(cx).await });
        let __arty_shutdown = __arty_runtime.stop();
        __arty_result
            .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
            .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
    }
    "#);
}

#[test]
fn test_preserves_attributes() {
    let expansion = test(
        TokenStream::new(),
        quote! {
            #[should_panic(expected = "original payload")]
            async fn fails(mut cx: arty::task::Context) {
                fail(&mut cx).await;
            }
        },
    );
    assert_snapshot!(render_expansion(&expansion), @r#"
    #[should_panic(expected = "original payload")]
    #[::core::prelude::v1::test]
    fn fails() {
        let __arty_runtime = ::arty::runtime::Runtime::builder()
            .monitor(::arty::runtime::MonitorPolicy::disabled())
            .build()
            .expect("failed to create the runtime for the entry point");
        let __arty_result = __arty_runtime
            .scheduler()
            .block_on(async move |mut cx: arty::task::Context| {
                fail(&mut cx).await;
            });
        let __arty_shutdown = __arty_runtime.stop();
        __arty_result
            .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
            .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
    }
    "#);
}

#[test]
fn workers_selects_exactly_the_literal_count() {
    let expansion = main(
        quote!(workers = 4usize, monitor = disabled),
        quote! {
            pub async fn run(cx: <App as Types>::Context) -> AppResult {
                run(cx).await
            }
        },
    );
    let expected = quote! {
        pub fn run() -> AppResult {
            let __arty_runtime = ::arty::runtime::Runtime::builder()
                .workers(::arty::runtime::WorkersPolicy::exactly(4usize))
                .monitor(::arty::runtime::MonitorPolicy::disabled())
                .build()
                .expect("failed to create the runtime for the entry point");
            let __arty_result = __arty_runtime
                .scheduler()
                .block_on(async move |cx: <App as Types>::Context| { run(cx).await });
            let __arty_shutdown = __arty_runtime.stop();
            __arty_result
                .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
        }
    };
    assert_eq!(expansion.to_string(), expected.to_string());
}

#[test]
fn all_workers_selects_all_available_processors() {
    let input = quote! {
        async fn run(cx: Context) {}
    };
    let main_expansion = main(quote!(workers = all), input.clone());
    let expected_main = quote! {
        fn run() {
            let __arty_runtime = ::arty::runtime::Runtime::builder()
                .workers(::arty::runtime::WorkersPolicy::all())
                .build()
                .expect("failed to create the runtime for the entry point");
            let __arty_result = __arty_runtime
                .scheduler()
                .block_on(async move |cx: Context| {});
            let __arty_shutdown = __arty_runtime.stop();
            __arty_result
                .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
        }
    };
    assert_eq!(main_expansion.to_string(), expected_main.to_string());

    let test_expansion = test(quote!(workers = all), input);
    let expected_test = quote! {
        #[::core::prelude::v1::test]
        fn run() {
            let __arty_runtime = ::arty::runtime::Runtime::builder()
                .workers(::arty::runtime::WorkersPolicy::all())
                .monitor(::arty::runtime::MonitorPolicy::disabled())
                .build()
                .expect("failed to create the runtime for the entry point");
            let __arty_result = __arty_runtime
                .scheduler()
                .block_on(async move |cx: Context| {});
            let __arty_shutdown = __arty_runtime.stop();
            __arty_result
                .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
        }
    };
    assert_eq!(test_expansion.to_string(), expected_test.to_string());
}

#[test]
fn controlled_test_can_select_all_workers() {
    let expansion = test(
        quote!(workers = all),
        quote! {
            async fn run(cx: Context, time: ClockControl) {}
        },
    );
    let expected = quote! {
        #[::core::prelude::v1::test]
        fn run() {
            let __arty_clock_control = ::arty::runtime::__private::ClockControl::new();
            let __arty_runtime = ::arty::runtime::Runtime::builder()
                .workers(::arty::runtime::WorkersPolicy::all())
                .monitor(::arty::runtime::MonitorPolicy::disabled())
                .clock(::core::clone::Clone::clone(&__arty_clock_control))
                .build()
                .expect("failed to create the runtime for the entry point");
            let __arty_result = __arty_runtime
                .scheduler()
                .block_on(async move |cx: Context| {
                    let time: ClockControl = __arty_clock_control;
                    {}
                });
            let __arty_shutdown = __arty_runtime.stop();
            __arty_result
                .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
        }
    };
    assert_eq!(expansion.to_string(), expected.to_string());
}

#[test]
fn controlled_test_shares_one_clock_and_preserves_qualified_types() {
    let expansion = test(
        quote!(monitor = enabled, workers = 1),
        quote! {
            #[ignore = "manual test"]
            #[should_panic(expected = "original payload")]
            async fn run(cx: <App as Types>::Context, mut time: <App as Types>::Control) {
                run(cx, &mut time).await;
            }
        },
    );
    let expected = quote! {
        #[ignore = "manual test"]
        #[should_panic(expected = "original payload")]
        #[::core::prelude::v1::test]
        fn run() {
            let __arty_clock_control = ::arty::runtime::__private::ClockControl::new();
            let __arty_runtime = ::arty::runtime::Runtime::builder()
                .workers(::arty::runtime::WorkersPolicy::exactly(1))
                .monitor(::arty::runtime::MonitorPolicy::enabled())
                .clock(::core::clone::Clone::clone(&__arty_clock_control))
                .build()
                .expect("failed to create the runtime for the entry point");
            let __arty_result = __arty_runtime
                .scheduler()
                .block_on(async move |cx: <App as Types>::Context| {
                    let mut time: <App as Types>::Control = __arty_clock_control;
                    {
                        run(cx, &mut time).await;
                    }
                });
            let __arty_shutdown = __arty_runtime.stop();
            __arty_result
                .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
        }
    };
    assert_eq!(expansion.to_string(), expected.to_string());
}

#[test]
fn controlled_test_without_workers_uses_the_runtime_default() {
    let expansion = test(
        TokenStream::new(),
        quote! {
            async fn run(cx: Context, time: ClockControl) {
                run(cx, time).await;
            }
        },
    );
    let expected = quote! {
        #[::core::prelude::v1::test]
        fn run() {
            let __arty_clock_control = ::arty::runtime::__private::ClockControl::new();
            let __arty_runtime = ::arty::runtime::Runtime::builder()
                .monitor(::arty::runtime::MonitorPolicy::disabled())
                .clock(::core::clone::Clone::clone(&__arty_clock_control))
                .build()
                .expect("failed to create the runtime for the entry point");
            let __arty_result = __arty_runtime
                .scheduler()
                .block_on(async move |cx: Context| {
                    let time: ClockControl = __arty_clock_control;
                    {
                        run(cx, time).await;
                    }
                });
            let __arty_shutdown = __arty_runtime.stop();
            __arty_result
                .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
        }
    };

    assert_eq!(expansion.to_string(), expected.to_string());
}

#[test]
fn main_monitor_settings_are_forwarded_exactly() {
    for (setting, policy) in [(quote!(enabled), quote!(enabled)), (quote!(disabled), quote!(disabled))] {
        let expansion = main(
            quote!(monitor = #setting),
            quote! {
                async fn run(cx: Context) {}
            },
        );
        let expected = quote! {
            fn run() {
                let __arty_runtime = ::arty::runtime::Runtime::builder()
                    .monitor(::arty::runtime::MonitorPolicy::#policy())
                    .build()
                    .expect("failed to create the runtime for the entry point");
                let __arty_result = __arty_runtime
                    .scheduler()
                    .block_on(async move |cx: Context| {});
                let __arty_shutdown = __arty_runtime.stop();
                __arty_result
                    .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                    .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
            }
        };

        assert_eq!(expansion.to_string(), expected.to_string(), "monitor = {setting}");
    }
}

#[test]
fn test_monitor_settings_are_forwarded_exactly() {
    for (setting, policy) in [(quote!(enabled), quote!(enabled)), (quote!(disabled), quote!(disabled))] {
        let expansion = test(
            quote!(monitor = #setting),
            quote! {
                async fn run(cx: Context) {}
            },
        );
        let expected = quote! {
            #[::core::prelude::v1::test]
            fn run() {
                let __arty_runtime = ::arty::runtime::Runtime::builder()
                    .monitor(::arty::runtime::MonitorPolicy::#policy())
                    .build()
                    .expect("failed to create the runtime for the entry point");
                let __arty_result = __arty_runtime
                    .scheduler()
                    .block_on(async move |cx: Context| {});
                let __arty_shutdown = __arty_runtime.stop();
                __arty_result
                    .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                    .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
            }
        };

        assert_eq!(expansion.to_string(), expected.to_string(), "monitor = {setting}");
    }
}

#[test]
fn main_panic_settings_are_forwarded_exactly() {
    for (policy, constant) in [(quote!(shutdown), quote!(SHUTDOWN)), (quote!(ignore), quote!(IGNORE))] {
        let expansion = main(
            quote!(panic = #policy),
            quote! {
                async fn run(cx: Context) {}
            },
        );
        let expected = quote! {
            fn run() {
                let __arty_runtime = ::arty::runtime::Runtime::builder()
                    .on(::arty::runtime::hook::Panic::#constant)
                    .build()
                    .expect("failed to create the runtime for the entry point");
                let __arty_result = __arty_runtime
                    .scheduler()
                    .block_on(async move |cx: Context| {});
                let __arty_shutdown = __arty_runtime.stop();
                __arty_result
                    .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                    .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
            }
        };

        assert_eq!(expansion.to_string(), expected.to_string(), "panic = {policy}");
    }
}

#[test]
fn test_panic_settings_are_forwarded_exactly() {
    for (policy, constant) in [(quote!(shutdown), quote!(SHUTDOWN)), (quote!(ignore), quote!(IGNORE))] {
        let expansion = test(
            quote!(panic = #policy),
            quote! {
                async fn run(cx: Context) {}
            },
        );
        let expected = quote! {
            #[::core::prelude::v1::test]
            fn run() {
                let __arty_runtime = ::arty::runtime::Runtime::builder()
                    .monitor(::arty::runtime::MonitorPolicy::disabled())
                    .on(::arty::runtime::hook::Panic::#constant)
                    .build()
                    .expect("failed to create the runtime for the entry point");
                let __arty_result = __arty_runtime
                    .scheduler()
                    .block_on(async move |cx: Context| {});
                let __arty_shutdown = __arty_runtime.stop();
                __arty_result
                    .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                    .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
            }
        };

        assert_eq!(expansion.to_string(), expected.to_string(), "panic = {policy}");
    }
}

#[test]
fn monitor_composes_independently_of_argument_order() {
    for expand in [main, test] {
        let input = quote! {
            async fn run(cx: Context) {}
        };
        let workers_first = expand(quote!(workers = 2, monitor = disabled), input.clone());
        let monitor_first = expand(quote!(monitor = disabled, workers = 2), input);

        assert_eq!(workers_first.to_string(), monitor_first.to_string());
    }
}

#[test]
fn panic_composes_with_other_settings_independently_of_argument_order() {
    for expand in [main, test] {
        let input = quote! {
            async fn run(cx: Context) {}
        };
        let panic_last = expand(quote!(workers = 2, monitor = disabled, panic = ignore), input.clone());
        let panic_first = expand(quote!(panic = ignore, monitor = disabled, workers = 2), input);

        assert_eq!(panic_last.to_string(), panic_first.to_string());
    }
}

#[test]
fn worker_literals_are_not_limited_to_the_macro_hosts_pointer_width() {
    let forwarded = proc_macro2::TokenTree::Group(proc_macro2::Group::new(proc_macro2::Delimiter::None, quote!(2)));
    for workers in [
        quote!(0),
        quote!(0x0usize),
        quote!(1),
        quote!(0x10usize),
        quote!(340282366920938463463374607431768211456),
        TokenStream::from(forwarded),
    ] {
        let expansion = main(
            quote!(workers = #workers),
            quote! {
                async fn run(cx: Context) {}
            },
        );
        assert!(!expansion.to_string().contains("compile_error"));
    }
}

#[test]
fn worker_policy_unwraps_nested_expression_groups() {
    for (value, expected) in [
        (quote!(4usize), quote!(::arty::runtime::WorkersPolicy::exactly(4usize))),
        (quote!(all), quote!(::arty::runtime::WorkersPolicy::all())),
    ] {
        let value = TokenTree::Group(Group::new(
            Delimiter::None,
            TokenStream::from(TokenTree::Group(Group::new(Delimiter::None, value))),
        ));
        let expansion = main(
            quote!(workers = #value),
            quote!(
                async fn run(cx: Context) {}
            ),
        );
        let expected = quote! {
            fn run() {
                let __arty_runtime = ::arty::runtime::Runtime::builder()
                    .workers(#expected)
                    .build()
                    .expect("failed to create the runtime for the entry point");
                let __arty_result = __arty_runtime
                    .scheduler()
                    .block_on(async move |cx: Context| {});
                let __arty_shutdown = __arty_runtime.stop();
                __arty_result
                    .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                    .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
            }
        };
        assert_eq!(expansion.to_string(), expected.to_string());
    }
}

#[test]
fn invalid_worker_counts_report_errors() {
    for workers in [
        quote!(-0),
        quote!(-1),
        quote!(-1usize),
        quote!(-0x10),
        quote!(1u32),
        quote!(1.5),
        quote!("2"),
        quote!("ALL"),
        quote!(""),
        quote!("all"),
        quote!(ALL),
        quote!(module::all),
        quote!(::all),
        quote!(all::<usize>),
        quote!(<T>::all),
        quote!(true),
        quote!(COUNT),
        quote!(1 + 1),
        quote!(WorkersPolicy::all()),
    ] {
        for expand in [main, test] {
            let expansion = expand(
                quote!(workers = #workers),
                quote! {
                    async fn run(cx: Context) {}
                },
            )
            .to_string();
            assert!(expansion.contains("compile_error"), "workers = {workers}: {expansion}");
            assert!(expansion.contains("nonnegative integer literal"), "{expansion}");
        }
    }
}

#[test]
fn invalid_monitor_settings_report_errors() {
    let expected = quote!(::core::compile_error! { "`monitor` must be the bare identifier `enabled` or `disabled`" }).to_string();
    for monitor in [
        quote!(true),
        quote!(false),
        quote!("enabled"),
        quote!("disabled"),
        quote!(automatic),
        quote!(MonitorPolicy::enabled()),
        quote!(::enabled),
    ] {
        for expand in [main, test] {
            let expansion = expand(
                quote!(monitor = #monitor),
                quote! {
                    async fn run(cx: Context) {}
                },
            )
            .to_string();

            assert_eq!(expansion, expected, "monitor = {monitor}");
        }
    }
}

#[test]
fn duplicate_monitor_settings_report_errors() {
    let expected = quote!(::core::compile_error! { "Duplicate field `monitor`" }).to_string();
    for expand in [main, test] {
        let expansion = expand(
            quote!(monitor = enabled, monitor = disabled),
            quote! {
                async fn run(cx: Context) {}
            },
        )
        .to_string();

        assert_eq!(expansion, expected);
    }
}

#[test]
fn invalid_panic_settings_report_errors() {
    let expected = quote!(::core::compile_error! { "`panic` must be the bare identifier `shutdown` or `ignore`" }).to_string();
    for panic in [
        quote!(true),
        quote!(false),
        quote!("shutdown"),
        quote!("ignore"),
        quote!(continue),
        quote!(abort),
        quote!(Panic::shutdown()),
        quote!(::shutdown),
    ] {
        for expand in [main, test] {
            let expansion = expand(
                quote!(panic = #panic),
                quote! {
                    async fn run(cx: Context) {}
                },
            )
            .to_string();

            assert_eq!(expansion, expected, "panic = {panic}");
        }
    }
}

#[test]
fn duplicate_panic_settings_report_errors() {
    let expected = quote!(::core::compile_error! { "Duplicate field `panic`" }).to_string();
    for expand in [main, test] {
        let expansion = expand(
            quote!(panic = shutdown, panic = ignore),
            quote! {
                async fn run(cx: Context) {}
            },
        )
        .to_string();

        assert_eq!(expansion, expected);
    }
}

#[test]
fn removed_configuration_options_are_rejected() {
    for args in [quote!(builder = configure()), quote!(runtime_path = ::renamed)] {
        for expand in [main, test] {
            let expansion = expand(
                args.clone(),
                quote! {
                    async fn run(cx: Context) {}
                },
            )
            .to_string();
            assert!(expansion.contains("compile_error"), "{expansion}");
            assert!(expansion.contains("Unknown field"), "{expansion}");
        }
    }
}

#[test]
fn injected_clock_requires_a_named_owned_second_test_parameter() {
    for input in [
        quote!(
            async fn run(cx: Context, _: ClockControl) {}
        ),
        quote!(
            async fn run(cx: Context, control: &ClockControl) {}
        ),
        quote!(
            async fn run(cx: Context, control: ClockControl, extra: ClockControl) {}
        ),
        quote!(
            async fn run(cx: Context, cx: ClockControl) {}
        ),
        quote!(
            async fn run(cx: Context, r#cx: ClockControl) {}
        ),
    ] {
        for expand in [main, test] {
            let expansion = expand(TokenStream::new(), input.clone()).to_string();
            assert!(expansion.starts_with(&input.to_string()));
            assert!(expansion.contains("compile_error"), "{expansion}");
        }
    }
    let expansion = main(
        TokenStream::new(),
        quote!(
            async fn run(cx: Context, control: ClockControl) {}
        ),
    )
    .to_string();
    assert!(expansion.contains("unexpected arguments"));
}

#[test]
fn invalid_signatures_preserve_input_and_report_errors() {
    let cases = [
        (
            quote!(
                fn run(cx: Context) {}
            ),
            "function must be async",
        ),
        (
            quote!(
                async fn run() {}
            ),
            "function must take an Arty task context argument",
        ),
        (
            quote!(
                async fn run(_: Context) {}
            ),
            "argument must have an identifier",
        ),
        (
            quote!(
                async fn run(cx: &Context) {}
            ),
            "arty::task::Context by value",
        ),
        (
            quote!(
                async fn run(cx: Context, extra: ()) {}
            ),
            "unexpected arguments",
        ),
    ];
    for (input, message) in cases {
        for expand in [main, test] {
            let expansion = expand(TokenStream::new(), input.clone()).to_string();
            assert!(expansion.starts_with(&input.to_string()));
            assert!(expansion.contains(message), "{expansion}");
            assert!(expansion.contains("compile_error"));
        }
    }
}

#[test]
fn malformed_items_and_arguments_report_errors() {
    for (args, input) in [
        (TokenStream::new(), quote!(not a function)),
        (
            quote!(@),
            quote!(
                async fn run(cx: Context) {}
            ),
        ),
        (
            quote!(unknown = 1),
            quote!(
                async fn run(cx: Context) {}
            ),
        ),
        (
            quote!(workers = 1, workers = 2),
            quote!(
                async fn run(cx: Context) {}
            ),
        ),
    ] {
        assert!(main(args, input).to_string().contains("compile_error"));
    }
}
