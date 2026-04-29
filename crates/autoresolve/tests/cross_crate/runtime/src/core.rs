/// Re-export of `xc_scheduler::core::Builtins`. The `#[reexport_base]`
/// macro emits a local helper module so downstream crates can refer to
/// `xc_runtime::core::Builtins` as if the base were defined here.
#[autoresolve::reexport_base(helper_module_exported_as = crate::core::builtins_helper)]
pub type Builtins = super::internal::Builtins;
