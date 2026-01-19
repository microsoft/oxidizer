// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A macro to generate `Fn` like wrapper types with consistent patterns.
///
/// This macro generates a type that wraps a function in an `Arc<dyn Fn...>`,
/// providing `Clone`, `Debug`, and convenient constructor methods. We need this to allow storing
/// user-provided functions (e.g., predicates) in a thread-safe, clonable way.
///
/// # Syntax
///
/// ```rust,ignore
/// define_fn_wrapper!(TypeName<Generics>(Fn(args) -> ReturnType));
/// ```
///
/// # Example
///
/// ```rust,ignore
/// define_fn_wrapper!(ShouldRetry<Res>(Fn(&Res, ShouldRetryArgs) -> Recovery));
/// ```
///
/// This generates a `ShouldRetry<Res>` struct with methods:
/// - `new<F>(predicate: F) -> Self` where `F: Fn(...) + Send + Sync + 'static`
/// - `call(&self, args...) -> ReturnType` to invoke the wrapped function
/// - `Clone` and `Debug` implementations
macro_rules! define_fn_wrapper {
    // Match pattern: Name<Generic>(Fn(param_name: param_type, ...) -> return_type)
    ($name:ident<$($generics:ident),*>(Fn($($param_name:ident: $param_ty:ty),*) -> $return_ty:ty)) => {
        pub(crate) struct $name<$($generics),*>(std::sync::Arc<dyn Fn($($param_ty),*) -> $return_ty + Send + Sync>);

        impl<$($generics),*> $name<$($generics),*> {
            pub(crate) fn new<F>(predicate: F) -> Self
            where
                F: Fn($($param_ty),*) -> $return_ty + Send + Sync + 'static,
            {
                Self(std::sync::Arc::new(predicate))
            }

            pub(crate) fn call(&self, $($param_name: $param_ty),*) -> $return_ty {
                (self.0)($($param_name),*)
            }
        }

        impl<$($generics),*> Clone for $name<$($generics),*> {
            fn clone(&self) -> Self {
                Self(self.0.clone())
            }
        }

        impl<$($generics),*> std::fmt::Debug for $name<$($generics),*> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish()
            }
        }
    };

    // Match pattern without return type (defaults to unit)
    ($name:ident<$($generics:ident),*>(Fn($($param_name:ident: $param_ty:ty),*))) => {
        $crate::define_fn_wrapper!($name<$($generics),*>(Fn($($param_name: $param_ty),*) -> ()));
    };

    // Alternative match for simple cases without explicit parameter names
    // For two parameters
    ($name:ident<$($generics:ident),*>(Fn($param1:ty, $param2:ty) -> $return_ty:ty)) => {
        $crate::define_fn_wrapper!($name<$($generics),*>(Fn(arg1: $param1, arg2: $param2) -> $return_ty));
    };

    // For two parameters without return type
    ($name:ident<$($generics:ident),*>(Fn($param1:ty, $param2:ty))) => {
        $crate::define_fn_wrapper!($name<$($generics),*>(Fn(arg1: $param1, arg2: $param2) -> ()));
    };

    // For one parameter
    ($name:ident<$($generics:ident),*>(Fn($param1:ty) -> $return_ty:ty)) => {
        $crate::define_fn_wrapper!($name<$($generics),*>(Fn(arg1: $param1) -> $return_ty));
    };

    // For one parameter without return type
    ($name:ident<$($generics:ident),*>(Fn($param1:ty))) => {
        $crate::define_fn_wrapper!($name<$($generics),*>(Fn(arg1: $param1) -> ()));
    };

    // For zero parameters
    ($name:ident<$($generics:ident),*>(Fn() -> $return_ty:ty)) => {
        $crate::define_fn_wrapper!($name<$($generics),*>(Fn() -> $return_ty));
    };

    // For zero parameters without return type
    ($name:ident<$($generics:ident),*>(Fn())) => {
        $crate::define_fn_wrapper!($name<$($generics),*>(Fn() -> ()));
    };

    // Match pattern without return type (defaults to unit)
    ($name:ident(Fn($($param_name:ident: $param_ty:ty),*))) => {
        $crate::define_fn_wrapper!($name(Fn($($param_name: $param_ty),*) -> ()));
    };

    // Alternative match for simple cases without explicit parameter names
    // For two parameters
    ($name:ident(Fn($param1:ty, $param2:ty) -> $return_ty:ty)) => {
        $crate::define_fn_wrapper!($name(Fn(arg1: $param1, arg2: $param2) -> $return_ty));
    };

    // For two parameters without return type
    ($name:ident(Fn($param1:ty, $param2:ty))) => {
        $crate::define_fn_wrapper!($name(Fn(arg1: $param1, arg2: $param2) -> ()));
    };

    // For one parameter
    ($name:ident(Fn($param1:ty) -> $return_ty:ty)) => {
        $crate::define_fn_wrapper!($name(Fn(arg1: $param1) -> $return_ty));
    };

    // For one parameter without return type
    ($name:ident(Fn($param1:ty))) => {
        $crate::define_fn_wrapper!($name(Fn(arg1: $param1) -> ()));
    };

    // For zero parameters
    ($name:ident(Fn() -> $return_ty:ty)) => {
        $crate::define_fn_wrapper!($name(Fn() -> $return_ty));
    };

    // For zero parameters without return type
    ($name:ident(Fn())) => {
        $crate::define_fn_wrapper!($name(Fn() -> ()));
    };
}

pub(crate) use define_fn_wrapper;

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    define_fn_wrapper!(InOut<In, Out>(Fn(&In) -> Out));

    #[test]
    fn static_assertions() {
        static_assertions::assert_impl_all!(InOut<String, String>: Send, Sync, Debug, Clone);
    }

    #[test]
    fn call_ok() {
        let wrapper = InOut::new(|input: &String| input.clone());

        let result = wrapper.call(&"Hello, World!".to_string());
        assert_eq!(result, "Hello, World!".to_string());

        let wrapper = wrapper;
        let result = wrapper.call(&"Hello, World!".to_string());
        assert_eq!(result, "Hello, World!".to_string());
    }

    #[test]
    fn debug_ok() {
        let wrapper = InOut::new(|input: &String| input.clone());

        let debug_str = format!("{wrapper:?}");

        assert_eq!(debug_str, "InOut");
    }
}
