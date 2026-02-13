use crate::resolve_deps::{ResolutionDeps, ResolutionDepsEnd, ResolutionDepsNode};

pub trait ResolveFrom<T>: Send + Sync + 'static {
    type Inputs: ResolutionDeps<T>;

    fn new(inputs: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> Self;
}

impl<T> ResolveFrom<T> for T
where
    T: AsRef<T> + Clone + Send + Sync + 'static,
{
    type Inputs = ResolutionDepsNode<T, T, ResolutionDepsEnd>;

    fn new(input: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> T {
        input.0.clone()
    }
}
