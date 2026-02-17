mod resolve_deps;
mod resolve_from;
mod resolver;

use std::marker::PhantomData;

pub use resolve_deps::{ResolutionDeps, ResolutionDepsEnd, ResolutionDepsNode};
pub use resolve_from::ResolveFrom;
pub use resolver::Resolver;

pub trait Resolvable {
    type Alternatives;
}

pub struct ResolutionAlternativesEnd;

pub struct ResolutionAlternativesNode<FirstAlternative, AlternativeTail>(PhantomData<(FirstAlternative, AlternativeTail)>);

pub struct OtherResolver<Base>(PhantomData<Base>);

impl<Base> OtherResolver<Base> {
    pub fn resolve<Output, Path>(&self) -> Output
    where
        Output: Resolvable,
        Output::Alternatives: CanResolveOneAlternativeFromBase<Base, Path>,
    {
        unimplemented!()
    }

    pub fn with_base<NewBase>(self) -> OtherResolver<ResolutionBaseListNode<NewBase, Base>> {
        OtherResolver(PhantomData)
    }
}

pub fn new_other_resolver<Base>() -> OtherResolver<ResolutionBaseListNode<Base, ResolutionBaseEnd>> {
    OtherResolver(PhantomData)
}

pub struct ResolutionBaseEnd;

pub struct ResolutionBaseListNode<Head, Tail>(Head, Tail);

pub struct ResolutionPathInHead;

pub struct ResolutionPathInHeadThrough<InnerPath>(PhantomData<InnerPath>);

pub struct ResolutionPathInTail<InnerPath>(PhantomData<InnerPath>);

pub trait ResolutionCoveredByBase<Base, Path> {}

impl<Head, Tail> ResolutionCoveredByBase<ResolutionBaseListNode<Head, Tail>, ResolutionPathInHead> for Head {}
impl<OtherHead, Head, Tail, InnerPath> ResolutionCoveredByBase<ResolutionBaseListNode<OtherHead, Tail>, ResolutionPathInTail<InnerPath>>
    for Head
where
    Head: ResolutionCoveredByBase<Tail, InnerPath>,
{
}

impl<Base, Resolv, InnerPath> ResolutionCoveredByBase<Base, ResolutionPathInHeadThrough<InnerPath>> for Resolv
where
    Resolv: Resolvable,
    Resolv::Alternatives: CanResolveOneAlternativeFromBase<Base, InnerPath>,
{
}

pub trait ResolutionOfAllCoveredByBase<Base, Path> {}

pub struct ResolutionPathNoResolution;

pub struct ResolutionPathResolvedHeadAndTail<HeadPath, TailPath>(PhantomData<(HeadPath, TailPath)>);

impl<Base> ResolutionOfAllCoveredByBase<Base, ResolutionPathNoResolution> for ResolutionBaseEnd {}

impl<Requested, RequestedTail, Base, RequestedPathInBase, RequestedTailPathInBase>
    ResolutionOfAllCoveredByBase<Base, ResolutionPathResolvedHeadAndTail<RequestedPathInBase, RequestedTailPathInBase>>
    for ResolutionBaseListNode<Requested, RequestedTail>
where
    Requested: ResolutionCoveredByBase<Base, RequestedPathInBase>,
    RequestedTail: ResolutionOfAllCoveredByBase<Base, RequestedTailPathInBase>,
{
}

pub trait CanResolveOneAlternativeFromBase<Base, ResolutionPath> {}

pub struct ResolutionPathFirstAlternative<InnerPath>(PhantomData<InnerPath>);
pub struct ResolutionPathOtherAlternative<InnerPath>(PhantomData<InnerPath>);

impl<Base, InnerPath, RequestedBase, AlternativeTail> CanResolveOneAlternativeFromBase<Base, ResolutionPathFirstAlternative<InnerPath>>
    for ResolutionAlternativesNode<RequestedBase, AlternativeTail>
where
    RequestedBase: ResolutionOfAllCoveredByBase<Base, InnerPath>,
{
}

impl<Base, InnerPath, AlternativeHead, AlternativeTail> CanResolveOneAlternativeFromBase<Base, ResolutionPathOtherAlternative<InnerPath>>
    for ResolutionAlternativesNode<AlternativeHead, AlternativeTail>
where
    AlternativeTail: CanResolveOneAlternativeFromBase<Base, InnerPath>,
{
}
