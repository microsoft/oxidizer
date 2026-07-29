// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Field-type classification used by the `#[resolver]` codegen.
//!
//! Maps a variant field's declared type to the coercion strategy the generated
//! code applies to its captured path value.

use syn::visit::Visit as _;
use syn::{GenericArgument, Lifetime, Path, PathArguments, TraitBound, Type, TypeBareFn};

/// The extraction strategy for a field, chosen from its declared type.
pub(crate) enum FieldKind {
    /// `&str` — raw, undecoded, borrowed (static routes only).
    Raw,
    /// `String` — percent-decoded, owned.
    Owned,
    /// `Cow<'_, str>` — decoded on demand.
    Cow,
    /// any `T: FromStr` — decoded then parsed.
    Parse,
}

/// Classifies a (non-`Option`) field type into its extraction strategy.
pub(crate) fn field_kind(ty: &Type) -> FieldKind {
    if is_str_reference(ty) {
        FieldKind::Raw
    } else if type_path_is_any(ty, &[&["String"], &["std", "string", "String"], &["alloc", "string", "String"]]) {
        FieldKind::Owned
    } else if type_path_is_any(ty, &[&["Cow"], &["std", "borrow", "Cow"], &["alloc", "borrow", "Cow"]]) {
        FieldKind::Cow
    } else {
        FieldKind::Parse
    }
}

/// Whether `ty` is an `Option<_>`.
pub(crate) fn is_option(ty: &Type) -> bool {
    option_inner(ty).is_some()
}

/// Whether `ty` is a shared reference to `str` (`&str` / `&'a str`), i.e. a
/// borrowing field. Dynamic route variants reject these.
pub(crate) fn is_str_reference(ty: &Type) -> bool {
    match ty {
        Type::Reference(reference) => matches!(&*reference.elem, Type::Path(p) if p.path.is_ident("str")),
        _ => false,
    }
}

/// Whether `ty` is exactly a shared `&'p str` capture borrow.
pub(crate) fn is_capture_str_reference(ty: &Type) -> bool {
    let Type::Reference(reference) = ty else {
        return false;
    };
    reference.mutability.is_none()
        && reference.lifetime.as_ref().is_some_and(|lifetime| lifetime.ident == "p")
        && matches!(&*reference.elem, Type::Path(path) if path.path.is_ident("str"))
}

/// Whether `ty` is exactly `Cow<'p, str>`, modulo path qualification.
pub(crate) fn is_capture_cow(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    if segment.ident != "Cow" {
        return false;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    let mut arguments = arguments.args.iter();
    matches!(arguments.next(), Some(GenericArgument::Lifetime(lifetime)) if lifetime.ident == "p")
        && matches!(
            arguments.next(),
            Some(GenericArgument::Type(Type::Path(path))) if path.path.segments.last().is_some_and(|segment| segment.ident == "str")
        )
        && arguments.next().is_none()
}

/// Whether `ty` mentions the request lifetime `'p`.
pub(crate) fn uses_capture_lifetime(ty: &Type) -> bool {
    struct Finder {
        found: bool,
        shadowed: usize,
    }

    impl<'ast> syn::visit::Visit<'ast> for Finder {
        fn visit_lifetime(&mut self, i: &'ast Lifetime) {
            self.found |= self.shadowed == 0 && i.ident == "p";
        }

        fn visit_type_bare_fn(&mut self, i: &'ast TypeBareFn) {
            let shadows = i.lifetimes.as_ref().is_some_and(bound_lifetimes_contain_p);
            self.shadowed += usize::from(shadows);
            syn::visit::visit_type_bare_fn(self, i);
            self.shadowed -= usize::from(shadows);
        }

        fn visit_trait_bound(&mut self, i: &'ast TraitBound) {
            let shadows = i.lifetimes.as_ref().is_some_and(bound_lifetimes_contain_p);
            self.shadowed += usize::from(shadows);
            syn::visit::visit_trait_bound(self, i);
            self.shadowed -= usize::from(shadows);
        }
    }

    let mut finder = Finder { found: false, shadowed: 0 };
    finder.visit_type(ty);
    finder.found
}

/// Returns the inner type `T` of an `Option<T>`, or `None` for other types.
fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else { return None };
    if !path_is_any(
        &type_path.path,
        &[&["Option"], &["std", "option", "Option"], &["core", "option", "Option"]],
    ) {
        return None;
    }
    let segment = type_path.path.segments.last()?;
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        GenericArgument::Type(inner) => Some(inner),
        _ => None,
    })
}

fn bound_lifetimes_contain_p(lifetimes: &syn::BoundLifetimes) -> bool {
    lifetimes
        .lifetimes
        .iter()
        .any(|param| matches!(param, syn::GenericParam::Lifetime(lifetime) if lifetime.lifetime.ident == "p"))
}

fn type_path_is_any(ty: &Type, candidates: &[&[&str]]) -> bool {
    matches!(ty, Type::Path(type_path) if type_path.qself.is_none() && path_is_any(&type_path.path, candidates))
}

fn path_is_any(path: &Path, candidates: &[&[&str]]) -> bool {
    candidates.iter().any(|candidate| {
        path.segments.len() == candidate.len()
            && path
                .segments
                .iter()
                .zip(candidate.iter())
                .all(|(segment, expected)| segment.ident == *expected)
    })
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    #[test]
    fn is_option_classifies_types() {
        assert!(is_option(&parse_quote!(Option<u32>)));
        assert!(!is_option(&parse_quote!(String)));
        // Defensive branches: `Option` without generics, and a non-type arg.
        assert!(!is_option(&parse_quote!(Option)));
        assert!(!is_option(&parse_quote!(Option<'a>)));
        assert!(!is_option(&parse_quote!(&'a str)));
    }

    #[test]
    fn field_kind_classifies_types() {
        assert!(matches!(field_kind(&parse_quote!(&'p str)), FieldKind::Raw));
        assert!(matches!(field_kind(&parse_quote!(String)), FieldKind::Owned));
        assert!(matches!(field_kind(&parse_quote!(std::borrow::Cow<'p, str>)), FieldKind::Cow));
        assert!(matches!(field_kind(&parse_quote!(u32)), FieldKind::Parse));
        assert!(matches!(field_kind(&parse_quote!(custom::String)), FieldKind::Parse));
        assert!(matches!(field_kind(&parse_quote!(custom::Cow<'p>)), FieldKind::Parse));
        assert!(!is_option(&parse_quote!(custom::Option<u32>)));
    }

    #[test]
    fn is_str_reference_detects_borrows() {
        assert!(is_str_reference(&parse_quote!(&'p str)));
        assert!(is_str_reference(&parse_quote!(&str)));
        assert!(!is_str_reference(&parse_quote!(String)));
        assert!(!is_str_reference(&parse_quote!(u32)));
    }

    #[test]
    fn capture_borrows_require_the_p_lifetime_and_shared_str_shape() {
        assert!(is_capture_str_reference(&parse_quote!(&'p str)));
        assert!(!is_capture_str_reference(&parse_quote!(&'static str)));
        assert!(!is_capture_str_reference(&parse_quote!(&'p mut str)));
        assert!(!is_capture_str_reference(&parse_quote!(String)));
        assert!(is_capture_cow(&parse_quote!(Cow<'p, str>)));
        assert!(is_capture_cow(&parse_quote!(std::borrow::Cow<'p, str>)));
        assert!(!is_capture_cow(&parse_quote!(Cow<'static, str>)));
        assert!(!is_capture_cow(&parse_quote!(Cow<'p, [u8]>)));
        assert!(!is_capture_cow(&parse_quote!(&'p str)));
        assert!(!is_capture_cow(&parse_quote!(String)));
        assert!(!is_capture_cow(&parse_quote!(Cow)));

        let mut path: syn::TypePath = parse_quote!(Cow<'p, str>);
        path.path.segments.clear();
        assert!(!is_capture_cow(&Type::Path(path)));
    }

    #[test]
    fn capture_lifetime_is_found_inside_field_types() {
        assert!(uses_capture_lifetime(&parse_quote!(Invariant<'p>)));
        assert!(uses_capture_lifetime(&parse_quote!(fn(&'p str))));
        assert!(!uses_capture_lifetime(&parse_quote!(for<'p> fn(&'p str))));
        assert!(!uses_capture_lifetime(&parse_quote!(dyn for<'p> Trait<'p>)));
        assert!(uses_capture_lifetime(&parse_quote!((&'p str, for<'p> fn(&'p str)))));
        assert!(uses_capture_lifetime(&parse_quote!((for<'p> fn(&'p str), &'p str))));
        assert!(uses_capture_lifetime(&parse_quote!((dyn for<'p> Trait<'p>, &'p str))));
        assert!(uses_capture_lifetime(&parse_quote!(dyn Trait<'p>)));
        assert!(uses_capture_lifetime(&parse_quote!(for<'a> fn(&'p str, &'a str))));
        assert!(!uses_capture_lifetime(&parse_quote!(Invariant<'static>)));
        assert!(!uses_capture_lifetime(&parse_quote!(String)));
    }
}
