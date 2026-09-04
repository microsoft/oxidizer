// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(dead_code)]

#[derive(Clone, Copy)]
pub struct Engines;

impl Engines {
    pub const ALL: Self = Self;
    pub const CRITERION: Self = Self;
    pub const DEFAULT: Self = Self;
}

pub trait BenchmarkCase: 'static {
    fn name(&self) -> String;
}

impl BenchmarkCase for () {
    fn name(&self) -> String {
        String::new()
    }
}

pub trait BenchmarkCases: Sized + 'static {
    type Case: BenchmarkCase;

    fn cases() -> impl IntoIterator<Item = Self::Case>;
}

pub trait Fixture: Sized + 'static {
    type Case: BenchmarkCase;

    fn cases() -> impl IntoIterator<Item = Self::Case>;
    fn setup(case: &Self::Case) -> Self;
}

pub trait SimpleFixture: Sized + 'static {
    fn setup() -> Self;
}

impl<T: SimpleFixture> Fixture for T {
    type Case = ();

    fn cases() -> impl IntoIterator<Item = Self::Case> {
        [()]
    }

    fn setup(_: &Self::Case) -> Self {
        <Self as SimpleFixture>::setup()
    }
}

pub struct Bencher;

impl Bencher {
    pub fn run<F, Output>(&mut self, _: F)
    where
        F: FnMut() -> Output,
    {
    }

    pub fn setup<F, State>(&mut self, _: F) -> SetupBencher<State>
    where
        F: FnMut() -> State,
    {
        SetupBencher(std::marker::PhantomData)
    }
}

pub struct SetupBencher<State>(std::marker::PhantomData<State>);

impl<State> SetupBencher<State> {
    pub fn run<F, Output>(&mut self, _: F)
    where
        F: FnMut(State) -> Output,
    {
    }
}

pub struct BenchmarkSuite {
    groups: Vec<BenchmarkGroup>,
    selected: Option<String>,
}

impl BenchmarkSuite {
    pub fn new(selected: Option<&str>) -> Self {
        Self {
            groups: Vec::new(),
            selected: selected.map(str::to_owned),
        }
    }

    pub fn benchmark_group(&mut self, name: impl Into<String>) -> &mut BenchmarkGroup {
        let name = name.into();
        if let Some(index) = self.groups.iter().position(|group| group.name == name) {
            return &mut self.groups[index];
        }
        self.groups.push(BenchmarkGroup {
            name,
            identities: Vec::new(),
            selected: self.selected.clone(),
        });
        self.groups.last_mut().unwrap()
    }

    pub fn identities(&self) -> Vec<&str> {
        self.groups
            .iter()
            .flat_map(|group| group.identities.iter().map(String::as_str))
            .collect()
    }
}

pub struct BenchmarkGroup {
    name: String,
    identities: Vec<String>,
    selected: Option<String>,
}

impl BenchmarkGroup {
    pub fn benchmark_case<F>(&mut self, name: impl Into<String>, case: Option<String>, _: Engines, _: F)
    where
        F: Fn(&mut Bencher) + 'static,
    {
        let name = name.into();
        let identity = match case {
            Some(case) => format!("{}/{name}/{case}", self.name),
            None => format!("{}/{name}", self.name),
        };
        if self.selected.as_deref().is_none_or(|selected| selected == identity) {
            self.identities.push(identity);
        }
    }
}

pub mod __private {
    pub use crate::BenchmarkSuite;

    pub trait BenchmarkGroupDefinition {
        fn register(suite: &mut crate::BenchmarkSuite);
    }

    pub struct PreparedOutput<Output, State>(Output, State);

    impl<Output, State> PreparedOutput<Output, State> {
        pub fn new(output: Output, state: State) -> Self {
            Self(output, state)
        }
    }
}
