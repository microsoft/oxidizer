// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Argument parsing for the fakeable attribute macro.

use proc_macro2::Span;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, LitStr, Path, Result, Token};

/// Arguments for the `#[fakeable(...)]` attribute macro.
#[derive(Debug, Clone)]
pub(super) struct FakeableArgs {
    /// The feature/cfg attribute name for enabling fake implementations.
    pub(super) fakes_feature: String,
    /// The path to the fake implementation type (for structs only).
    pub(super) fake_impl: Option<Path>,
    /// The name of the constructor method for creating fake instances (for structs only).
    pub(super) fake_constructor: Option<String>,
    /// Whether to generate mockall fake (for impl blocks only).
    pub(super) generate_mockall_fake: Option<bool>,
    /// The module where mockall fake should be generated (for impl blocks only).
    pub(super) mockall_fake_module: Option<String>,
}

impl Parse for FakeableArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut fakes_feature = None;
        let mut fake_impl = None;
        let mut fake_constructor = None;
        let mut generate_mockall_fake = None;
        let mut mockall_fake_module = None;

        let args: Punctuated<FakeableArg, Token![,]> = Punctuated::parse_terminated(input)?;

        for arg in args {
            match arg {
                FakeableArg::FakesFeature(value) => {
                    if fakes_feature.is_some() {
                        return Err(syn::Error::new_spanned(value, "fakes_feature specified multiple times"));
                    }
                    fakes_feature = Some(value);
                }
                FakeableArg::FakeImpl(path) => {
                    if fake_impl.is_some() {
                        return Err(syn::Error::new_spanned(path, "fake_impl specified multiple times"));
                    }
                    fake_impl = Some(path);
                }
                FakeableArg::FakeConstructor(name) => {
                    if fake_constructor.is_some() {
                        return Err(syn::Error::new_spanned(name, "fake_constructor specified multiple times"));
                    }
                    fake_constructor = Some(name);
                }
                FakeableArg::GenerateMockallFake(enabled) => {
                    if generate_mockall_fake.is_some() {
                        return Err(syn::Error::new(Span::call_site(), "generate_mockall_fake specified multiple times"));
                    }
                    generate_mockall_fake = Some(enabled);
                }
                FakeableArg::MockallFakeModule(module) => {
                    if mockall_fake_module.is_some() {
                        return Err(syn::Error::new(Span::call_site(), "mockall_fake_module specified multiple times"));
                    }
                    mockall_fake_module = Some(module);
                }
            }
        }

        // Default to "test-util" if not specified, per M-TEST-UTIL:
        // https://microsoft.github.io/rust-guidelines/guidelines/libs/resilience/index.html#M-TEST-UTIL
        let fakes_feature = fakes_feature.unwrap_or_else(|| "test-util".to_string());

        Ok(Self {
            fakes_feature,
            fake_impl,
            fake_constructor,
            generate_mockall_fake,
            mockall_fake_module,
        })
    }
}

#[derive(Debug, Clone)]
enum FakeableArg {
    FakesFeature(String),
    FakeImpl(Path),
    FakeConstructor(String),
    GenerateMockallFake(bool),
    MockallFakeModule(String),
}

impl Parse for FakeableArg {
    fn parse(input: ParseStream) -> Result<Self> {
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;

        match name.to_string().as_str() {
            "fakes_feature" | "fakes_attribute" => {
                let value: LitStr = input.parse()?;
                Ok(Self::FakesFeature(value.value()))
            }
            "fake_impl" => {
                let path: Path = input.parse()?;
                Ok(Self::FakeImpl(path))
            }
            "fake_constructor" => {
                let value: LitStr = input.parse()?;
                Ok(Self::FakeConstructor(value.value()))
            }
            "generate_mockall_fake" => {
                // Only accept boolean true
                let lookahead = input.lookahead1();
                if lookahead.peek(syn::LitBool) {
                    let value: syn::LitBool = input.parse()?;
                    if value.value {
                        Ok(Self::GenerateMockallFake(true))
                    } else {
                        Err(syn::Error::new_spanned(value, "generate_mockall_fake must be true"))
                    }
                } else {
                    Err(syn::Error::new(input.span(), "generate_mockall_fake must be true"))
                }
            }
            "mockall_fake_module" => {
                let value: LitStr = input.parse()?;
                Ok(Self::MockallFakeModule(value.value()))
            }
            _ => Err(syn::Error::new_spanned(
                name,
                "unknown argument; expected fakes_feature (or fakes_attribute), fake_impl, fake_constructor, generate_mockall_fake, or mockall_fake_module",
            )),
        }
    }
}
