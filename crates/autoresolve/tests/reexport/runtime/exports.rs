use autoresolve::reexport_base;

#[reexport_base(helper_module_exported_as = crate::runtime::exports::builtins_helper)]
pub type Builtins = super::middle::Builtins;