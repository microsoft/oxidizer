#![expect(
    missing_debug_implementations,
    clippy::empty_structs_with_brackets,
    clippy::must_use_candidate,
    reason = "Unit tests"
)]

// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub struct Logger {}
pub struct Database {}

impl Database {
    // Some dependency asked for by normal reference
    pub const fn new(_: &Logger) -> Self {
        Self {}
    }
}

// #[fundle::bundle]
// pub struct AppState {
//     logger: Logger,
//     database: Database,
// }

fn main() {
    let _ = AppState::builder()
        .logger(|_| Logger {})
        .database(|x| {
            Database::new(x.logger())})
        .build();
}


#[allow(non_camel_case_types, non_snake_case)]
pub struct AppState {
    logger: Logger,
    database: Database,
}
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl AppState {
    pub fn builder() -> AppStateBuilder<::fundle::Write, ::fundle::NotSet, ::fundle::NotSet> { AppStateBuilder::default() }
}
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl ::std::convert::AsRef<Logger> for AppState {
    fn as_ref(&self) -> &Logger { &self.logger }
}
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl ::std::convert::AsRef<Database> for AppState {
    fn as_ref(&self) -> &Database { &self.database }
}
impl ::fundle::exports::Exports for AppState {
    const NUM_EXPORTS: usize = 2usize;
}
#[allow(clippy::items_after_statements)]
impl ::fundle::exports::Export<0usize> for AppState {
    type T = Logger;
    fn get(&self) -> &Self::T { &self.logger }
}
#[allow(clippy::items_after_statements)]
impl ::fundle::exports::Export<1usize> for AppState {
    type T = Database;
    fn get(&self) -> &Self::T { &self.database }
}
#[allow(non_snake_case)]
mod _AppState {
    use super::*;
    #[allow(non_camel_case_types, dead_code, non_snake_case, clippy::items_after_statements)]
    pub struct AppStateBuilder<RW, LOGGER, DATABASE> {
        logger: ::std::option::Option<Logger>,
        database: ::std::option::Option<Database>,
        _phantom: ::std::marker::PhantomData<(RW, LOGGER, DATABASE)>,
    }
    #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
    impl ::std::default::Default for AppStateBuilder<::fundle::Write, ::fundle::NotSet, ::fundle::NotSet> { fn default() -> Self { Self { logger: ::std::option::Option::None, database: ::std::option::Option::None, _phantom: ::std::marker::PhantomData } } }

    #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
    impl<LOGGER, DATABASE> ::fundle::Writer for AppStateBuilder<::fundle::Write, LOGGER, DATABASE> { type Reader = AppStateBuilder<::fundle::Read, LOGGER, DATABASE>; }

    #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
    impl<LOGGER, DATABASE> ::fundle::Reader for AppStateBuilder<::fundle::Read, LOGGER, DATABASE> { type Writer = AppStateBuilder<::fundle::Write, LOGGER, DATABASE>; }

    #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
    impl<LOGGER, DATABASE> AppStateBuilder<::fundle::Write, LOGGER, DATABASE> {
        #[doc(hidden)]
        pub fn read(self) -> AppStateBuilder<::fundle::Read, LOGGER, DATABASE> {
            AppStateBuilder {
                logger: self.logger,
                database: self.database,
                _phantom: ::std::marker::PhantomData,
            }
        }
    }
    #[allow(non_camel_case_types, non_snake_case)]
    impl<DATABASE> AppStateBuilder<::fundle::Write, ::fundle::NotSet, DATABASE> {
        pub fn logger(self, f: impl ::std::ops::Fn(&<Self as ::fundle::Writer>::Reader) -> Logger) -> AppStateBuilder<::fundle::Write, ::fundle::Set, DATABASE> {
            let read = self.read();
            let logger = f(&read);
            AppStateBuilder {
                logger: ::std::option::Option::Some(logger),
                database: read.database,
                _phantom: ::std::marker::PhantomData,
            }
        }
        pub fn logger_try<R: ::std::error::Error>(self, f: impl ::std::ops::Fn(&<Self as ::fundle::Writer>::Reader) -> ::std::result::Result<Logger,
            R>) -> ::std::result::Result<AppStateBuilder<::fundle::Write, ::fundle::Set, DATABASE>, R> {
            let read = self.read();
            let logger = f(&read)?;
            ::std::result::Result::Ok(AppStateBuilder {
                logger: ::std::option::Option::Some(logger),
                database: read.database,
                _phantom: ::std::marker::PhantomData,
            })
        }
        pub async fn logger_try_async<F, R: ::std::error::Error>(self, f: F) -> ::std::result::Result<AppStateBuilder<::fundle::Write, ::fundle::Set, DATABASE>, R>
        where
            F: AsyncFn(&<Self as ::fundle::Writer>::Reader) -> ::std::result::Result<Logger,
                R>,
        {
            let read = self.read();
            let logger = f(&read).await?;
            ::std::result::Result::Ok(AppStateBuilder {
                logger: ::std::option::Option::Some(logger),
                database: read.database,
                _phantom: ::std::marker::PhantomData,
            })
        }
        pub async fn logger_async<F>(self, f: F) -> AppStateBuilder<::fundle::Write, ::fundle::Set, DATABASE>
        where
            F: AsyncFn(&<Self as ::fundle::Writer>::Reader) -> Logger,
        {
            let read = self.read();
            let logger = f(&read).await;
            AppStateBuilder {
                logger: ::std::option::Option::Some(logger),
                database: read.database,
                _phantom: ::std::marker::PhantomData,
            }
        }
    }
    #[allow(non_camel_case_types, non_snake_case)]
    impl<LOGGER> AppStateBuilder<::fundle::Write, LOGGER, ::fundle::NotSet> {
        pub fn database(self, f: impl ::std::ops::Fn(&<Self as ::fundle::Writer>::Reader) -> Database) -> AppStateBuilder<::fundle::Write, LOGGER, ::fundle::Set> {
            let read = self.read();
            let database = f(&read);
            AppStateBuilder {
                logger: read.logger,
                database: ::std::option::Option::Some(database),
                _phantom: ::std::marker::PhantomData,
            }
        }
        pub fn database_try<R: ::std::error::Error>(self, f: impl ::std::ops::Fn(&<Self as ::fundle::Writer>::Reader) -> ::std::result::Result<Database,
            R>) -> ::std::result::Result<AppStateBuilder<::fundle::Write, LOGGER, ::fundle::Set>, R> {
            let read = self.read();
            let database = f(&read)?;
            ::std::result::Result::Ok(AppStateBuilder {
                logger: read.logger,
                database: ::std::option::Option::Some(database),
                _phantom: ::std::marker::PhantomData,
            })
        }
        pub async fn database_try_async<F, R: ::std::error::Error>(self, f: F) -> ::std::result::Result<AppStateBuilder<::fundle::Write, LOGGER, ::fundle::Set>, R>
        where
            F: AsyncFn(&<Self as ::fundle::Writer>::Reader) -> ::std::result::Result<Database,
                R>,
        {
            let read = self.read();
            let database = f(&read).await?;
            ::std::result::Result::Ok(AppStateBuilder {
                logger: read.logger,
                database: ::std::option::Option::Some(database),
                _phantom: ::std::marker::PhantomData,
            })
        }
        pub async fn database_async<F>(self, f: F) -> AppStateBuilder<::fundle::Write, LOGGER, ::fundle::Set>
        where
            F: AsyncFn(&<Self as ::fundle::Writer>::Reader) -> Database,
        {
            let read = self.read();
            let database = f(&read).await;
            AppStateBuilder {
                logger: read.logger,
                database: ::std::option::Option::Some(database),
                _phantom: ::std::marker::PhantomData,
            }
        }
    }
    #[allow(non_camel_case_types, non_snake_case)]
    impl<DATABASE> AppStateBuilder<::fundle::Read, ::fundle::Set, DATABASE> { pub fn logger(&self) -> &Logger { self.logger.as_ref().unwrap() } }

    #[allow(non_camel_case_types, non_snake_case)]
    impl<LOGGER> AppStateBuilder<::fundle::Read, LOGGER, ::fundle::Set> { pub fn database(&self) -> &Database { self.database.as_ref().unwrap() } }
    #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
    impl<RW, DATABASE> ::std::convert::AsRef<Logger> for AppStateBuilder<RW, ::fundle::Set, DATABASE> { fn as_ref(&self) -> &Logger { self.logger.as_ref().unwrap() } }
    #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
    impl<RW, LOGGER> ::std::convert::AsRef<Database> for AppStateBuilder<RW, LOGGER, ::fundle::Set> { fn as_ref(&self) -> &Database { self.database.as_ref().unwrap() } }
    #[allow(non_camel_case_types, non_snake_case)]
    impl<RW, DATABASE> ::fundle::exports::Export<0usize> for AppStateBuilder<RW, ::fundle::Set, DATABASE> {
        type T = Logger;
        fn get(&self) -> &Self::T { self.logger.as_ref().unwrap() }
    }
    #[allow(non_camel_case_types, non_snake_case)]
    impl<RW, LOGGER> ::fundle::exports::Export<1usize> for AppStateBuilder<RW, LOGGER, ::fundle::Set> {
        type T = Database;
        fn get(&self) -> &Self::T { self.database.as_ref().unwrap() }
    }
    #[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
    impl AppStateBuilder<::fundle::Write, ::fundle::Set, ::fundle::Set> {
        pub fn build(self) -> AppState {
            AppState {
                logger: self.logger.unwrap(),
                database: self.database.unwrap(),
            }
        }
    }
}
pub use _AppState::AppStateBuilder;
#[allow(unused_macros, snake_case)] macro_rules! AppState {
     ( verify_field   $   builder_var   :   ident   logger ) =>   { { fn   verify_exists   <   RW   ,   T2   >   ( _   :   &   AppStateBuilder   <   RW   ,   ::   fundle   ::   Set   ,   T2   >   ) {  } verify_exists   ( $   builder_var   ) ;   } } ;   ( verify_field   $   builder_var   :   ident   database ) =>   { { fn   verify_exists   <   RW   ,   T1   >   ( _   :   &   AppStateBuilder   <   RW   ,   T1   ,   ::   fundle   ::   Set   >   ) {  } verify_exists   ( $   builder_var   ) ;   } } ;   ( select   ( $   builder_var   :   ident   ) =>   $   ( $   forward_type   :   ident   ( $   forward_field   :   ident   ) ) ,   *   $   ( ,   ) ?   ) =>   { { $   ( AppState   !   ( verify_field   $   builder_var   $   forward_field   ) ;   ) *   #   [ allow   ( non_camel_case_types   ,   non_snake_case   ,   clippy   ::   items_after_statements   ) ] struct   Select   <   'a   ,   RW   ,   T1   ,   T2   >   { builder   :   &   'a   AppStateBuilder   <   RW   ,   T1   ,   T2   >   ,   $   ( $   forward_type   :   &   'a   $   forward_type   ,   ) *   } impl   <   'a   ,   RW   ,   T2   >   ::   std   ::   convert   ::   AsRef   <   Logger   >   for   Select   <   'a   ,   RW   ,   ::   fundle   ::   Set   ,   T2   >   where   AppStateBuilder   <   RW   ,   ::   fundle   ::   Set   ,   T2   >   :   ::   std   ::   convert   ::   AsRef   <   Logger   >   ,   { fn   as_ref   ( &   self   ) ->   &   Logger { self   .   builder   .   as_ref   (  ) } } impl   <   'a   ,   RW   ,   T1   >   ::   std   ::   convert   ::   AsRef   <   Database   >   for   Select   <   'a   ,   RW   ,   T1   ,   ::   fundle   ::   Set   >   where   AppStateBuilder   <   RW   ,   T1   ,   ::   fundle   ::   Set   >   :   ::   std   ::   convert   ::   AsRef   <   Database   >   ,   { fn   as_ref   ( &   self   ) ->   &   Database { self   .   builder   .   as_ref   (  ) } } $   ( #   [ allow   ( non_camel_case_types   ,   non_snake_case   ,   clippy   ::   items_after_statements   ) ] impl   <   'a   ,   RW   ,   T1   ,   T2   >   ::   std   ::   convert   ::   AsRef   <   $   forward_type   >   for   Select   <   'a   ,   RW   ,   T1   ,   T2   >   { fn   as_ref   ( &   self   ) ->   &   $   forward_type   { self   .   $   forward_type   } } ) *   Select   { builder   :   &   $   builder_var   ,   $   ( $   forward_type   :   $   builder_var   .   $   forward_field   (  ) ,   ) *   } } } ;   }