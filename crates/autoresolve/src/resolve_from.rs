use crate::resolver::Resolver;

pub struct ResolutionDepsEnd;

pub struct ResolutionDepsNode<H, T>(pub H, pub T);

trait ResolutionDepsPrivate<T>: Send + Sync + 'static {
    type ResolvedPrivate<'a>
    where
        Self: 'a,
        T: 'a;

    fn ensure(base: &mut Resolver<T>);

    fn get_private(base: &Resolver<T>) -> Self::ResolvedPrivate<'_>;
}

#[expect(private_bounds)]
pub trait ResolutionDeps<T>: ResolutionDepsPrivate<T> {
    type Resolved<'a>
    where
        Self: 'a,
        T: 'a;

    fn get(base: &mut Resolver<T>) -> Self::Resolved<'_>;
}

impl<T> ResolutionDepsPrivate<T> for ResolutionDepsEnd {
    type ResolvedPrivate<'a>
        = ResolutionDepsEnd
    where
        Self: 'a,
        T: 'a;

    fn ensure(_base: &mut Resolver<T>) {}

    fn get_private(_base: &Resolver<T>) -> Self::ResolvedPrivate<'_> {
        ResolutionDepsEnd
    }
}

impl<T, H, TList> ResolutionDepsPrivate<T> for ResolutionDepsNode<H, TList>
where
    H: ResolveFrom<T>,
    TList: ResolutionDeps<T>,
    T: Send + Sync + 'static,
{
    type ResolvedPrivate<'a>
        = ResolutionDepsNode<&'a H, TList::ResolvedPrivate<'a>>
    where
        Self: 'a,
        T: 'a;
    fn get_private(base: &Resolver<T>) -> Self::ResolvedPrivate<'_> {
        let tail = TList::get_private(base);
        let head = base.try_get::<H>().expect("Ensure must have been called before new");
        ResolutionDepsNode(head, tail)
    }

    fn ensure(base: &mut Resolver<T>) {
        base.ensure::<H>();
        TList::ensure(base);
    }
}

impl<T> ResolutionDeps<T> for ResolutionDepsEnd {
    type Resolved<'a>
        = ResolutionDepsEnd
    where
        Self: 'a,
        T: 'a;

    fn get(base: &mut Resolver<T>) -> Self::Resolved<'_> {
        Self::ensure(base);
        Self::get_private(base)
    }
}

impl<T, H, TList> ResolutionDeps<T> for ResolutionDepsNode<H, TList>
where
    TList: ResolutionDeps<T>,
    for<'a> TList: ResolutionDepsPrivate<T, ResolvedPrivate<'a> = TList::Resolved<'a>>,
    H: ResolveFrom<T>,
    T: Send + Sync + 'static,
{
    type Resolved<'a>
        = ResolutionDepsNode<&'a H, TList::Resolved<'a>>
    where
        Self: 'a,
        T: 'a;

    fn get(base: &mut Resolver<T>) -> Self::Resolved<'_> {
        Self::ensure(base);
        Self::get_private(base)
    }
}

pub trait ResolveFrom<T>: Send + Sync + 'static {
    type Inputs: ResolutionDeps<T>;

    fn new(inputs: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> Self;
}

impl<T> ResolveFrom<T> for T
where
    T: Clone + Send + Sync + 'static,
{
    type Inputs = ResolutionDepsNode<T, ResolutionDepsEnd>;

    fn new(input: <Self::Inputs as ResolutionDeps<T>>::Resolved<'_>) -> T {
        let ResolutionDepsNode(value, ResolutionDepsEnd) = input;
        value.clone()
    }
}
