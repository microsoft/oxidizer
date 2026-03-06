use crate::resolve_deps::ResolutionDeps;

pub trait ResolveFrom<T: 'static>: Send + Sync + 'static {
    type Inputs: ResolutionDeps<T>;

    fn new(inputs: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> Self;
}
