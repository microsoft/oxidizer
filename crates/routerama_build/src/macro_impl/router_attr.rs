// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::string::{String, ToString as _};
use alloc::vec::Vec;

use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned as _;
use syn::visit::Visit as _;
use syn::{GenericParam, Ident, Token, Type};

#[cfg(feature = "route")]
#[derive(Default)]
pub(super) struct RouterAttr {
    pub(super) state: Option<Type>,
    pub(super) erased_mounts: bool,
    pub(super) tower: bool,
    pub(super) heterogeneous_data: bool,
}

#[cfg(feature = "route")]
impl Parse for RouterAttr {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(Self::default());
        }

        let mut state = None;
        let mut erased_mounts = None;
        let mut tower = None;
        let mut heterogeneous_data = None;
        while !input.is_empty() {
            let key: Ident = input
                .parse()
                .map_err(|_error| input.error("expected `state = StateType`, `erased_mounts`, `tower`, or `heterogeneous_data`"))?;
            if key == "state" {
                if state.is_some() {
                    return Err(syn::Error::new(key.span(), "duplicate `state` router argument"));
                }
                let _equals: Token![=] = input.parse()?;
                let state_type: Type = input
                    .parse()
                    .map_err(|_error| input.error("expected a concrete shared state type after `state =`"))?;
                validate_router_state_type(&state_type)?;
                state = Some(state_type);
            } else if key == "erased_mounts" {
                if erased_mounts.replace(key.span()).is_some() {
                    return Err(syn::Error::new(key.span(), "duplicate `erased_mounts` router argument"));
                }
            } else if key == "tower" {
                if tower.replace(key.span()).is_some() {
                    return Err(syn::Error::new(key.span(), "duplicate `tower` router argument"));
                }
            } else if key == "heterogeneous_data" {
                if heterogeneous_data.replace(key.span()).is_some() {
                    return Err(syn::Error::new(key.span(), "duplicate `heterogeneous_data` router argument"));
                }
            } else {
                return Err(syn::Error::new(
                    key.span(),
                    "unknown router argument; expected `state = StateType`, `erased_mounts`, `tower`, or `heterogeneous_data`",
                ));
            }

            if input.is_empty() {
                break;
            }
            let _comma: Token![,] = input.parse()?;
            if input.is_empty() {
                break;
            }
        }

        if let Some(span) = erased_mounts
            && state.is_none()
        {
            return Err(syn::Error::new(
                span,
                "`erased_mounts` requires a fixed `state = StateType` router contract",
            ));
        }

        Ok(Self {
            state,
            erased_mounts: erased_mounts.is_some(),
            tower: tower.is_some(),
            heterogeneous_data: heterogeneous_data.is_some(),
        })
    }
}

#[cfg(feature = "route")]
fn validate_router_state_type(state: &Type) -> syn::Result<()> {
    let mut bound_lifetimes = BoundLifetimeCollector::default();
    bound_lifetimes.visit_type(state);

    let mut validator = RouterStateTypeValidator {
        bound_lifetimes: &bound_lifetimes.names,
        callable_depth: 0,
        error: None,
    };
    validator.visit_type(state);
    match validator.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(feature = "route")]
#[derive(Default)]
struct BoundLifetimeCollector {
    names: Vec<String>,
}

#[cfg(feature = "route")]
impl<'ast> syn::visit::Visit<'ast> for BoundLifetimeCollector {
    fn visit_bound_lifetimes(&mut self, i: &'ast syn::BoundLifetimes) {
        self.names.extend(i.lifetimes.iter().filter_map(|parameter| {
            let GenericParam::Lifetime(lifetime) = parameter else {
                return None;
            };
            Some(lifetime.lifetime.ident.to_string())
        }));
        syn::visit::visit_bound_lifetimes(self, i);
    }
}

#[cfg(feature = "route")]
struct RouterStateTypeValidator<'a> {
    bound_lifetimes: &'a [String],
    callable_depth: usize,
    error: Option<syn::Error>,
}

#[cfg(feature = "route")]
impl RouterStateTypeValidator<'_> {
    fn reject(&mut self, span: proc_macro2::Span, message: &'static str) {
        if self.error.is_none() {
            self.error = Some(syn::Error::new(span, message));
        }
    }
}

#[cfg(feature = "route")]
impl<'ast> syn::visit::Visit<'ast> for RouterStateTypeValidator<'_> {
    fn visit_type_impl_trait(&mut self, i: &'ast syn::TypeImplTrait) {
        self.reject(i.span(), "router state must be a concrete type and cannot use `impl Trait`");
    }

    fn visit_type_infer(&mut self, i: &'ast syn::TypeInfer) {
        self.reject(i.span(), "router state must be a fully specified type and cannot contain `_`");
    }

    fn visit_expr_infer(&mut self, i: &'ast syn::ExprInfer) {
        self.reject(
            i.span(),
            "router state must be a fully specified type and cannot contain an inferred const `_`",
        );
    }

    fn visit_type_macro(&mut self, i: &'ast syn::TypeMacro) {
        self.reject(
            i.span(),
            "router state must be a directly written concrete type and cannot use a type macro",
        );
    }

    fn visit_type_never(&mut self, i: &'ast syn::TypeNever) {
        self.reject(i.span(), "router state cannot use the uninhabited `!` type");
    }

    fn visit_type_trait_object(&mut self, i: &'ast syn::TypeTraitObject) {
        if i.dyn_token.is_none() {
            self.reject(i.span(), "trait-object router state types must use `dyn Trait + 'static`");
            return;
        }
        if !i
            .bounds
            .iter()
            .any(|bound| matches!(bound, syn::TypeParamBound::Lifetime(lifetime) if lifetime.ident == "static"))
        {
            self.reject(i.span(), "trait-object router state types require an explicit `+ 'static` lifetime");
            return;
        }
        syn::visit::visit_type_trait_object(self, i);
    }

    fn visit_path(&mut self, i: &'ast syn::Path) {
        if i.segments.first().is_some_and(|segment| segment.ident == "Self") {
            self.reject(
                i.span(),
                "router state types cannot use `Self`; name the annotated service or a fully qualified associated type explicitly",
            );
            return;
        }
        syn::visit::visit_path(self, i);
    }

    fn visit_type_reference(&mut self, i: &'ast syn::TypeReference) {
        if i.lifetime.is_none() && self.callable_depth == 0 {
            self.reject(i.span(), "references in a router state type require an explicit `'static` lifetime");
            return;
        }
        syn::visit::visit_type_reference(self, i);
    }

    fn visit_lifetime(&mut self, i: &'ast syn::Lifetime) {
        let name = i.ident.to_string();
        if name == "_" {
            self.reject(
                i.span(),
                "router state types cannot contain `'_`; use an owned type or an explicit `'static` reference",
            );
        } else if name != "static" && !self.bound_lifetimes.contains(&name) {
            self.reject(
                i.span(),
                "router state types cannot capture a local lifetime; use an owned type or an explicit `'static` reference",
            );
        }
    }

    fn visit_type_fn_ptr(&mut self, i: &'ast syn::TypeFnPtr) {
        self.callable_depth += 1;
        syn::visit::visit_type_fn_ptr(self, i);
        self.callable_depth -= 1;
    }

    fn visit_path_arguments(&mut self, i: &'ast syn::PathArguments) {
        if matches!(i, syn::PathArguments::Parenthesized(_)) {
            self.callable_depth += 1;
            syn::visit::visit_path_arguments(self, i);
            self.callable_depth -= 1;
        } else {
            syn::visit::visit_path_arguments(self, i);
        }
    }
}

#[cfg(all(test, feature = "route"))]
mod tests {
    use quote::quote;

    use super::*;

    #[test]
    #[cfg(feature = "route")]
    fn router_accepts_bare_fixed_tower_and_explicit_erased_mount_contracts() {
        let bare = syn::parse2::<RouterAttr>(quote! {}).expect("bare router");
        assert!(bare.state.is_none());
        assert!(!bare.erased_mounts);
        assert!(!bare.tower);
        for valid in [
            quote! { state = AppState },
            quote! { state = self::state::AppState<super::Dependency> },
            quote! { state = <Api as StateProvider>::State },
            quote! { state = &'static AppState, },
            quote! { state = for<'a> fn(&'a AppState) },
            quote! { state = fn(&AppState) },
            quote! { state = dyn StateContract + 'static },
            quote! { state = str },
            quote! { state = [u8] },
        ] {
            assert!(
                syn::parse2::<RouterAttr>(valid)
                    .expect("the state type is concrete and has no local anonymous lifetime")
                    .state
                    .is_some()
            );
        }

        for valid in [
            quote! { state = AppState, erased_mounts },
            quote! { erased_mounts, state = AppState, },
        ] {
            let mounted = syn::parse2::<RouterAttr>(valid).expect("erased mounts have fixed state");
            assert!(mounted.state.is_some());
            assert!(mounted.erased_mounts);
        }

        for valid in [
            quote! { tower },
            quote! { state = AppState, tower },
            quote! { tower, state = AppState, erased_mounts },
            quote! { heterogeneous_data },
            quote! { state = AppState, tower, heterogeneous_data },
        ] {
            let tower = syn::parse2::<RouterAttr>(valid).expect("the Tower adapter is an explicit additive contract");
            assert!(tower.tower || tower.heterogeneous_data);
        }
    }

    #[test]
    #[cfg(feature = "route")]
    fn router_rejects_unknown_duplicate_and_trailing_arguments() {
        for (invalid, expected) in [
            (quote! { context = AppState }, "unknown router argument"),
            (quote! { state = AppState, state = Other }, "duplicate `state`"),
            (quote! { state = AppState, extra = Other }, "unknown router argument"),
            (quote! { state = AppState,, }, "expected `state"),
            (quote! { state AppState }, "expected `=`"),
            (quote! { erased_mounts }, "requires a fixed `state"),
            (
                quote! { erased_mounts, erased_mounts, state = AppState },
                "duplicate `erased_mounts`",
            ),
            (quote! { tower, tower }, "duplicate `tower`"),
            (quote! { heterogeneous_data, heterogeneous_data }, "duplicate `heterogeneous_data`"),
        ] {
            let error = syn::parse2::<RouterAttr>(invalid)
                .err()
                .expect("malformed router arguments are rejected");
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    #[cfg(feature = "route")]
    fn router_rejects_non_concrete_or_ambiguous_state_types() {
        for (invalid, expected) in [
            (quote! { state = impl Clone }, "cannot use `impl Trait`"),
            (quote! { state = _ }, "cannot contain `_`"),
            (quote! { state = Wrapper<_> }, "cannot contain `_`"),
            (quote! { state = &'_ AppState }, "cannot contain `'_`"),
            (quote! { state = &AppState }, "explicit `'static`"),
            (quote! { state = AppState<'_> }, "cannot contain `'_`"),
            (quote! { state = make_state_type!() }, "cannot use a type macro"),
            (quote! { state = ! }, "cannot use the uninhabited"),
            (quote! { state = dyn StateContract }, "explicit `+ 'static`"),
            (quote! { state = StateContract + Send }, "must use `dyn Trait"),
            (quote! { state = Self }, "cannot use `Self`"),
        ] {
            let error = syn::parse2::<RouterAttr>(invalid)
                .err()
                .expect("non-concrete router state is rejected");
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
}
