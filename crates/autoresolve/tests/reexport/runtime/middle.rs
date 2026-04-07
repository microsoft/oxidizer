use autoresolve::reexport_base;

#[reexport_base(helper_module_exported_as = crate::runtime::middle::builtins_helper)]
pub type Builtins = super::internal::Builtins;