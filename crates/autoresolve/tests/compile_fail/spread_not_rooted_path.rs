use autoresolve_macros::base;

// Error: #[spread] field type must be a module-qualified path.
#[base]
mod base_single_segment {
    pub struct Base {
        #[spread]
        pub builtins: Builtins,
    }
}

// Error: #[spread] field type path must start with `super` or `crate`.
#[base]
mod base_unrooted {
    pub struct Base {
        #[spread]
        pub builtins: builtins_mod::builtins::Builtins,
    }
}

fn main() {}
