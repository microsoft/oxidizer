// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Adds explicit lifetimes to a method signature for mockall compatibility.
/// This is needed because mockall cannot handle elided lifetimes in generic type arguments.
pub(super) fn add_explicit_lifetimes(sig: &syn::Signature) -> syn::Signature {
    let mut sig = sig.clone();

    let needs_lifetime = sig.inputs.iter().any(|input| {
        if let syn::FnArg::Typed(syn::PatType { ty, .. }) = input {
            contains_reference_in_generic(ty)
        } else {
            false
        }
    });

    if !needs_lifetime {
        return sig;
    }

    let mut existing_lifetimes = std::collections::HashSet::new();
    for param in &sig.generics.params {
        if let syn::GenericParam::Lifetime(lifetime) = param {
            existing_lifetimes.insert(lifetime.lifetime.ident.to_string());
        }
    }

    let mut lifetime_name = None;
    for suffix in 0..=existing_lifetimes.len() {
        let candidate = if suffix == 0 { "mock".to_string() } else { format!("mock{suffix}") };
        if !existing_lifetimes.contains(&candidate) {
            lifetime_name = Some(candidate);
            break;
        }
    }
    let lifetime_name = lifetime_name.expect("one more candidate than existing lifetimes guarantees a free name");

    let lifetime: syn::LifetimeParam = syn::parse_str(&format!("'{lifetime_name}")).expect("constructed from a valid Rust identifier");
    let lifetime_value = lifetime.lifetime.clone();
    let mut insert_at = sig.generics.params.len();
    for (index, param) in sig.generics.params.iter().enumerate() {
        if !matches!(param, syn::GenericParam::Lifetime(_)) {
            insert_at = index;
            break;
        }
    }
    sig.generics.params.insert(insert_at, syn::GenericParam::Lifetime(lifetime));

    // Add the lifetime to all elided references in generic type arguments
    for input in &mut sig.inputs {
        if let syn::FnArg::Typed(syn::PatType { ty, .. }) = input {
            add_lifetime_to_references(ty, &lifetime_value);
        }
    }

    sig
}

/// Checks if a type contains references within generic arguments.
fn contains_reference_in_generic(ty: &syn::Type) -> bool {
    count_nested_elided_references(ty, false) > 0
}

pub(super) fn count_nested_elided_references(ty: &syn::Type, nested: bool) -> usize {
    struct NestedReferenceVisitor {
        nested: bool,
        count: usize,
    }

    impl<'ast> syn::visit::Visit<'ast> for NestedReferenceVisitor {
        fn visit_type(&mut self, i: &'ast syn::Type) {
            if let syn::Type::Reference(reference) = i
                && self.nested
                && reference.lifetime.is_none()
            {
                self.count += 1;
            }

            let parent_nested = self.nested;
            self.nested = true;
            syn::visit::visit_type(self, i);
            self.nested = parent_nested;
        }
    }

    let mut visitor = NestedReferenceVisitor { nested, count: 0 };
    syn::visit::Visit::visit_type(&mut visitor, ty);
    visitor.count
}

/// Adds a lifetime to all elided references within generic type arguments.
fn add_lifetime_to_references(ty: &mut syn::Type, lifetime: &syn::Lifetime) {
    add_lifetime_to_nested_references(ty, lifetime, false);
}

fn add_lifetime_to_nested_references(ty: &mut syn::Type, lifetime: &syn::Lifetime, nested: bool) {
    struct LifetimeVisitor<'a> {
        nested: bool,
        lifetime: &'a syn::Lifetime,
    }

    impl syn::visit_mut::VisitMut for LifetimeVisitor<'_> {
        fn visit_type_mut(&mut self, i: &mut syn::Type) {
            if let syn::Type::Reference(reference) = i
                && self.nested
                && reference.lifetime.is_none()
            {
                reference.lifetime = Some(self.lifetime.clone());
            }

            let parent_nested = self.nested;
            self.nested = true;
            syn::visit_mut::visit_type_mut(self, i);
            self.nested = parent_nested;
        }
    }

    let mut visitor = LifetimeVisitor { nested, lifetime };
    syn::visit_mut::VisitMut::visit_type_mut(&mut visitor, ty);
}

#[cfg(feature = "mockall")]
pub(super) fn type_contains_bound_lifetimes(ty: &syn::Type) -> bool {
    struct BoundLifetimeVisitor {
        found: bool,
    }

    impl<'ast> syn::visit::Visit<'ast> for BoundLifetimeVisitor {
        fn visit_bound_lifetimes(&mut self, _i: &'ast syn::BoundLifetimes) {
            self.found = true;
        }
    }

    let mut visitor = BoundLifetimeVisitor { found: false };
    syn::visit::Visit::visit_type(&mut visitor, ty);
    visitor.found
}

#[cfg(feature = "mockall")]
pub(super) fn type_contains_implicit_higher_ranked_elision(ty: &syn::Type) -> bool {
    struct ElidedReferenceVisitor {
        found: bool,
    }

    impl<'ast> syn::visit::Visit<'ast> for ElidedReferenceVisitor {
        fn visit_type_reference(&mut self, i: &'ast syn::TypeReference) {
            if i.lifetime.is_none() {
                self.found = true;
                return;
            }
            syn::visit::visit_type_reference(self, i);
        }
    }

    struct ImplicitHigherRankedElisionVisitor {
        found: bool,
    }

    impl<'ast> syn::visit::Visit<'ast> for ImplicitHigherRankedElisionVisitor {
        fn visit_type_fn_ptr(&mut self, i: &'ast syn::TypeFnPtr) {
            if i.lifetimes.is_none() {
                let mut visitor = ElidedReferenceVisitor { found: false };
                syn::visit::Visit::visit_type_fn_ptr(&mut visitor, i);
                self.found |= visitor.found;
            }
            syn::visit::visit_type_fn_ptr(self, i);
        }

        fn visit_trait_bound(&mut self, i: &'ast syn::TraitBound) {
            if i.lifetimes.is_none()
                && i.path
                    .segments
                    .last()
                    .is_some_and(|segment| matches!(segment.ident.to_string().as_str(), "Fn" | "FnMut" | "FnOnce"))
            {
                let mut visitor = ElidedReferenceVisitor { found: false };
                syn::visit::Visit::visit_trait_bound(&mut visitor, i);
                self.found |= visitor.found;
            }
            syn::visit::visit_trait_bound(self, i);
        }
    }

    let mut visitor = ImplicitHigherRankedElisionVisitor { found: false };
    syn::visit::Visit::visit_type(&mut visitor, ty);
    visitor.found
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use quote::ToTokens;
    use syn::{Type, parse_quote};

    use super::{add_explicit_lifetimes, add_lifetime_to_references, contains_reference_in_generic};

    #[test]
    fn detects_references_nested_in_generic_arguments() {
        let nested: Type = parse_quote!(Option<Vec<&str>>);
        let below_reference: Type = parse_quote!(&Option<&str>);
        let without_reference: Type = parse_quote!(Option<Vec<String>>);
        let direct: Type = parse_quote!(&str);

        assert!(contains_reference_in_generic(&nested));
        assert!(contains_reference_in_generic(&below_reference));
        assert!(!contains_reference_in_generic(&without_reference));
        assert!(!contains_reference_in_generic(&direct));
    }

    #[test]
    fn adds_lifetimes_below_direct_references() {
        let mut ty: Type = parse_quote!(&Option<&str>);

        add_lifetime_to_references(&mut ty, &parse_quote!('mock));

        assert_eq!(ty.into_token_stream().to_string(), "& Option < & 'mock str >");
    }

    #[test]
    fn adds_lifetimes_recursively_and_ignores_unrelated_types() {
        let mut nested: Type = parse_quote!(Option<Vec<&str>>);
        let mut explicit: Type = parse_quote!(Option<&'static str>);
        let mut lifetime_argument: Type = parse_quote!(Container<'static>);
        let mut tuple: Type = parse_quote!((u8, u16));

        add_lifetime_to_references(&mut nested, &parse_quote!('mock));
        add_lifetime_to_references(&mut explicit, &parse_quote!('mock));
        add_lifetime_to_references(&mut lifetime_argument, &parse_quote!('mock));
        add_lifetime_to_references(&mut tuple, &parse_quote!('mock));

        assert_eq!(nested.into_token_stream().to_string(), "Option < Vec < & 'mock str > >");
        assert_eq!(explicit.into_token_stream().to_string(), "Option < & 'static str >");
        assert_eq!(lifetime_argument.into_token_stream().to_string(), "Container < 'static >");
        assert_eq!(tuple.into_token_stream().to_string(), "(u8 , u16)");
    }

    #[test]
    fn preserves_existing_explicit_lifetimes() {
        let signature: syn::Signature = parse_quote!(fn value<'a>(&self, input: Option<&'a str>));

        let converted = add_explicit_lifetimes(&signature);

        assert_eq!(converted.into_token_stream().to_string(), signature.into_token_stream().to_string());
    }

    #[test]
    fn adds_noncolliding_lifetime_alongside_existing_lifetimes() {
        let signature: syn::Signature = parse_quote!(
            fn value<'mock, 'a>(
                &self,
                direct: &'a str,
                nested: Option<&str>,
            )
        );

        let converted = add_explicit_lifetimes(&signature);

        assert_eq!(
            converted.into_token_stream().to_string(),
            "fn value < 'mock , 'a , 'mock1 > (& self , direct : & 'a str , nested : Option < & 'mock1 str > ,)"
        );
    }

    #[test]
    fn inserts_mock_lifetime_before_type_parameters() {
        let signature: syn::Signature = parse_quote!(
            fn value<'a, T>(
                &self,
                direct: &'a str,
                nested: Option<&str>,
                value: T,
            )
        );

        let converted = add_explicit_lifetimes(&signature);

        assert_eq!(
            converted.into_token_stream().to_string(),
            "fn value < 'a , 'mock , T > (& self , direct : & 'a str , nested : Option < & 'mock str > , value : T ,)"
        );
    }
}
