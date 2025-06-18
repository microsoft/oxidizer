// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::{Item, ItemMacro, LitStr, parse_quote};

use crate::syn_helpers::token_stream_and_error;

struct Options {
    /// Marks the API as unstable and its presence is conditional on the specified feature flag.
    /// The feature flag is the value of this option, plus the "unstable-" prefix.
    unstable: String,

    /// If set, suppresses generation of the documentation comments on feature stability.
    /// e.g. because it is on a re-export where the underlying type is already documented
    skip_doc: bool,
}

impl Options {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Convention for syn-based code"
    )]
    fn parse(attr: TokenStream) -> Result<Self, syn::Error> {
        let mut unstable: Option<String> = None;
        let mut skip_doc = false;

        syn::meta::parser(|meta| {
            if matches!(meta.path.get_ident(), Some(ident) if ident == "skip_doc") {
                skip_doc = true;
                return Ok(());
            }

            if !meta.path.is_ident("unstable") {
                return Err(meta.error("unsupported argument"));
            }

            let unstable_feature_name: LitStr = meta.value()?.parse()?;

            if unstable.is_some() {
                return Err(meta.error("`unstable` argument specified multiple times"));
            }

            unstable = Some(unstable_feature_name.value());

            Ok(())
        })
        .parse2(attr.clone())?;

        Ok(Self {
            unstable: unstable
                .ok_or_else(|| syn::Error::new(attr.span(), "`unstable` argument is required"))?,
            skip_doc,
        })
    }
}

#[must_use]
pub fn entrypoint(attr: TokenStream, input: TokenStream) -> TokenStream {
    let item = syn::parse2::<Item>(input.clone());

    let result = match item {
        Ok(Item::Const(item)) => core(attr, item),
        Ok(Item::Enum(item)) => core(attr, item),
        Ok(Item::Fn(item)) => core(attr, item),
        Ok(Item::Macro(item)) => core_macro(attr, item),
        Ok(Item::Mod(item)) => core(attr, item),
        Ok(Item::Static(item)) => core(attr, item),
        Ok(Item::Struct(item)) => core(attr, item),
        Ok(Item::Trait(item)) => core(attr, item),
        Ok(Item::Type(item)) => core(attr, item),
        Ok(Item::Use(item)) => core(attr, item),
        Ok(x) => Err(syn::Error::new(
            x.span(),
            "the `unstable` attribute is not supported on this kind of item",
        )),
        Err(e) => Err(e),
    };

    match result {
        Ok(r) => r,
        Err(e) => token_stream_and_error(input, e),
    }
}

fn unstable_feature_name(options: &Options) -> String {
    // Unstable feature names are always prefixed with "unstable-" to be clear and unambiguous
    // about what they mean. This prefix does not need to be specified in the attributes themselves.
    format!("unstable-{}", options.unstable)
}

fn core(attr: TokenStream, mut item: impl SupportedItem) -> Result<TokenStream, syn::Error> {
    item.validate()?;

    let options = Options::parse(attr)?;
    let unstable_feature_name = unstable_feature_name(&options);

    if !options.skip_doc {
        document(&mut item, &unstable_feature_name);
    }

    // We emit two identical versions of the item, one with public, one with private visibility.
    // Based on feature flags, only one of these gets compiled.
    let private_item = item.to_private();

    let extended = quote! {
        #[cfg(feature = #unstable_feature_name)]
        #item

        #[cfg(not(feature = #unstable_feature_name))]
        #[allow(dead_code)]
        #[allow(unused_imports)]
        #private_item
    };

    Ok(extended)
}

fn core_macro(attr: TokenStream, mut item: ItemMacro) -> Result<TokenStream, syn::Error> {
    let options = Options::parse(attr)?;
    let unstable_feature_name = unstable_feature_name(&options);

    if !options.skip_doc {
        document(&mut item, &unstable_feature_name);
    }

    Ok(item.to_token_stream())
}

fn document(item: &mut impl DocumentableItem, unstable_feature_name: &str) {
    if core::option_env!("_CI__HIDE_UNSTABLE_DOCS_").is_some() {
        item.hide_doc();
        return;
    }

    item.push_doc(format!(
        include_str!("unstable-feature-notice.md"),
        unstable_feature_name = unstable_feature_name
    ));
}

/// We need to implement the attribute for different types of items, each of which may require
/// some specific logic to wire up all the metadata. This trait is implemented by all items that
/// we support.
trait SupportedItem: DocumentableItem + ToTokens {
    /// Validates that the item has no conflicting metadata that we cannot work with.
    fn validate(&self) -> Result<(), syn::Error>;

    /// Convert the item to a version of itself without public visibility.
    fn to_private(&self) -> Self;
}

trait DocumentableItem {
    /// Adds a documentation comment line to the end of any existing documentation.
    fn push_doc(&mut self, message: String);
    /// Marks existing documentation as hidden.
    fn hide_doc(&mut self);
}

macro_rules! support_item {
    ($($item:ty),+ $(,)?) => {
        $(
            impl SupportedItem for $item {
                fn validate(&self) -> Result<(), syn::Error> {
                    if (self.vis != parse_quote! { pub }) {
                        return Err(syn::Error::new(self.span(), "the `stability` attribute can only be added to `pub` items"));
                    }

                    Ok(())
                }

                fn to_private(&self) -> Self {
                    let mut private = self.clone();
                    private.vis = parse_quote! { pub(crate) };
                    private
                }
            }

            impl DocumentableItem for $item {
                fn push_doc(&mut self, message: String) {
                    self.attrs.push(parse_quote! { #[doc = #message] });
                }
                fn hide_doc(&mut self) {
                    self.attrs.push(parse_quote! { #[doc(hidden)] });
                }
            }
        )*
    };
}

support_item! {
    syn::ItemConst,
    syn::ItemEnum,
    syn::ItemFn,
    syn::ItemMod,
    syn::ItemStatic,
    syn::ItemStruct,
    syn::ItemTrait,
    syn::ItemType,
    syn::ItemUse,
}

impl DocumentableItem for ItemMacro {
    fn push_doc(&mut self, message: String) {
        self.attrs.push(parse_quote! { #[doc = #message] });
    }
    #[cfg_attr(test, mutants::skip)] // It is messy to unit test code which accesses env variables, for this alone the mutation testing of hide_doc is disabled.
    fn hide_doc(&mut self) {
        self.attrs.push(parse_quote! { #[doc(hidden)] });
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! __macro_unstable_pub_mod {
    ($mod_name:ident, $feature:literal) => {
        #[cfg(feature = $feature)]
        pub mod $mod_name;

        #[cfg(not(feature = $feature))]
        #[allow(dead_code, unused_imports)]
        pub(crate) mod $mod_name;
    };

    ($mod_name:ident, $( $feature:literal ),+) => {
        #[cfg(any($(feature = $feature),*))]
        pub mod $mod_name;

        #[cfg(not(any($(feature = $feature),*)))]
        #[allow(dead_code, unused_imports)]
        pub(crate) mod $mod_name;
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syn_helpers::contains_compile_error;

    #[test]
    fn smoke_test() {
        let attr = quote! { unstable = "foo", skip_doc };
        let input = quote! {
            /// Yolo wowza.
            pub struct Foo {
            }
        };

        let expected = quote! {
            #[cfg(feature = "unstable-foo")]
            /// Yolo wowza.
            pub struct Foo {
            }

            #[cfg(not(feature = "unstable-foo"))]
            #[allow(dead_code)]
            #[allow(unused_imports)]
            /// Yolo wowza.
            pub(crate) struct Foo {
            }
        };

        let result = entrypoint(attr, input);
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn api_is_documented() {
        let attr = quote! { unstable = "foo" };
        let input = quote! {
            /// Yolo wowza.
            pub struct Foo {
            }
        };

        let result = entrypoint(attr, input);
        let result = result.to_string();

        assert!(result.contains("This API is unstable."));
        assert!(result.contains("Yolo wowza."));
    }

    #[test]
    fn macro_api_is_documented() {
        let attr = quote! { unstable = "foo" };
        let input = quote! {
            /// Yolo wowza.
            macro_rules! foo {
                () => {
                    println!("foo");
                }
            }
        };

        let result = entrypoint(attr, input);
        let result = result.to_string();

        assert!(result.contains("This API is unstable."));
        assert!(result.contains("Yolo wowza."));
    }

    #[test]
    fn different_types_test() {
        let attr = quote! { unstable = "foo" };

        // We just check here that there is no compile error in the token stream.
        // The smoke test already covers the basic structure of the output.
        let inputs = vec![
            quote! {
                pub const FOO: i32 = 42;
            },
            quote! {
                pub enum Foo {
                    Bar,
                }
            },
            quote! {
                pub fn foo() {
                }
            },
            quote! {
                macro_rules! foo {
                    () => {
                        println!("foo");
                    }
                }
            },
            quote! {
                pub mod foo {
                }
            },
            quote! {
                pub static FOO: i32 = 42;
            },
            quote! {
                pub struct Foo {
                }
            },
            quote! {
                pub trait Foo {
                }
            },
            quote! {
                pub type Foo = i32;
            },
            quote! {
                pub use foo::*;
            },
        ];

        for input in inputs {
            let result = entrypoint(attr.clone(), input);
            assert!(!contains_compile_error(&result));
        }
    }

    #[test]
    fn macro_smoke_test() {
        // This is slightly pointless because all we do with macros is document the facts
        // so skip_doc (which is too annoying to test) means we really do nothing at all.
        let attr = quote! { unstable = "foo", skip_doc };
        let input = quote! {
            /// Yolo wowza.
            macro_rules! foo {
                () => {
                    println!("foo");
                }
            }
        };

        let expected = quote! {
            /// Yolo wowza.
            macro_rules! foo {
                () => {
                    println!("foo");
                }
            }
        };

        let result = entrypoint(attr, input);
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn extra_args_is_error() {
        let attr = quote! { unstable = "foo", skip_doc, random = value };
        let input = quote! {
            pub struct Foo {
            }
        };

        let result = entrypoint(attr, input);
        assert!(contains_compile_error(&result));
    }

    #[test]
    fn lack_of_args_is_error() {
        let input = quote! {
            pub struct Foo {
            }
        };

        let result = entrypoint(TokenStream::new(), input);
        assert!(contains_compile_error(&result));
    }

    #[test]
    fn duplicate_args_is_error() {
        let attr = quote! { unstable = "foo", unstable = "bar" };
        let input = quote! {
            pub struct Foo {
            }
        };

        let result = entrypoint(attr, input);
        assert!(contains_compile_error(&result));
    }

    #[test]
    fn not_pub_is_error() {
        let attr = quote! { unstable = "foo", skip_doc };
        let input = quote! {
            /// Yolo wowza.
            struct Foo {
            }
        };

        let result = entrypoint(attr, input);
        assert!(contains_compile_error(&result));
    }

    #[test]
    fn unsupported_item_is_error() {
        let attr = quote! { unstable = "foo", skip_doc };
        let input = quote! {
            extern crate foo;
        };

        let result = entrypoint(attr, input);
        assert!(contains_compile_error(&result));
    }

    #[test]
    fn nonsense_is_error() {
        let attr = quote! { unstable = "foo", skip_doc };
        let input = quote! {
            this is not even valid code,,,,
        };

        let result = entrypoint(attr, input);
        assert!(contains_compile_error(&result));
    }
}