use crate::ResolveFrom;
use crate::resolver_store::ResolverStore;

pub struct ResolutionDepsEnd;

pub struct ResolutionDepsNode<H, T>(pub H, pub T);

pub trait ResolutionDeps<T: 'static>: Send + Sync + 'static {
    type Resolved<'a>
    where
        Self: 'a,
        T: 'a;

    fn ensure<S: ResolverStore<T>>(store: &mut S);

    fn get_private<S: ResolverStore<T>>(store: &S) -> Self::Resolved<'_>;

    fn get<S: ResolverStore<T>>(store: &mut S) -> Self::Resolved<'_> {
        Self::ensure(store);
        Self::get_private(store)
    }
}

impl<T: 'static> ResolutionDeps<T> for ResolutionDepsEnd {
    type Resolved<'a>
        = ResolutionDepsEnd
    where
        Self: 'a,
        T: 'a;

    fn ensure<S: ResolverStore<T>>(_store: &mut S) {}

    fn get_private<S: ResolverStore<T>>(_store: &S) -> Self::Resolved<'_> {
        ResolutionDepsEnd
    }
}

impl<T, H, Rest> ResolutionDeps<T> for ResolutionDepsNode<H, Rest>
where
    H: ResolveFrom<T>,
    Rest: ResolutionDeps<T>,
    T: Send + Sync + 'static,
{
    type Resolved<'a>
        = ResolutionDepsNode<&'a H, Rest::Resolved<'a>>
    where
        Self: 'a,
        T: 'a;
    fn get_private<S: ResolverStore<T>>(store: &S) -> Self::Resolved<'_> {
        let tail = Rest::get_private(store);
        let head = store.lookup::<H>().expect("ensure must have been called before get_private");
        ResolutionDepsNode(head, tail)
    }

    fn ensure<S: ResolverStore<T>>(store: &mut S) {
        store.resolve::<H>();
        Rest::ensure(store);
    }
}
