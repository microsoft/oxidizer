use autoresolve_macros::base;

#[derive(Clone)]
pub struct Scheduler;

#[base]
mod app_base {
    pub struct AppBase {
        pub scheduler: super::Scheduler,
    }
}

// Error: scoped parent must be module-qualified (e.g., `app_base::AppBase`).
#[base(scoped(AppBase))]
mod request_base {
    pub struct RequestBase {}
}

fn main() {}
