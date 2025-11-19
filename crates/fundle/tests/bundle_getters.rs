// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    unused_attributes,
    clippy::empty_structs_with_brackets,
    clippy::redundant_type_annotations,
    clippy::items_after_statements,
    reason = "Unit tests"
)]

#[derive(Default, Clone)]
struct Logger {}
#[derive(Default, Clone)]
struct Config {}
#[derive(Default, Clone)]
struct Telemetry {}

mod gpu {
    #[derive(Clone, Default)]
    pub struct Instance;
    #[derive(Clone, Default)]
    pub struct Device;
    #[derive(Clone, Default)]
    pub struct Vulkan;

    #[fundle::bundle]
    #[derive(Default)]
    pub struct GpuBundle {
        instance: Instance,
        device: Device,
        vulkan: Vulkan,
    }
}

// #[fundle::bundle]
// struct AppState {
//     logger1: Logger,
//     logger2: Logger,
//     config: Config,
//     telemetry: Telemetry,
//     #[forward(gpu::Instance, gpu::Device, gpu::Vulkan)]
//     gpu: gpu::GpuBundle,
// }

#[test]
fn f() {
    AppState::builder()
        .logger1(|x| Logger::default())
        .logger2(|x| Logger::default())
        .telemetry(|x| {
            let asd = x.logger2();
            Telemetry::default()
        });
}


#[allow(non_camel_case_types, non_snake_case)]
struct AppState {
    logger1: Logger,
    logger2: Logger,
    config: Config,
    telemetry: Telemetry,
    gpu: gpu::GpuBundle,
}
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl AppState {
    pub fn builder() -> AppStateBuilder<::fundle::NotSet, ::fundle::NotSet, ::fundle::NotSet, ::fundle::NotSet, ::fundle::NotSet> { AppStateBuilder::default() }
}
#[allow(non_camel_case_types, dead_code, non_snake_case, clippy::items_after_statements)]
struct AppStateBuilder<LOGGER1, LOGGER2, CONFIG, TELEMETRY, GPU> {
    logger1: ::std::option::Option<Logger>,
    logger2: ::std::option::Option<Logger>,
    config: ::std::option::Option<Config>,
    telemetry: ::std::option::Option<Telemetry>,
    gpu: ::std::option::Option<gpu::GpuBundle>,
    _phantom: ::std::marker::PhantomData<(LOGGER1, LOGGER2, CONFIG, TELEMETRY, GPU)>,
}

#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl ::std::default::Default for AppStateBuilder<::fundle::NotSet, ::fundle::NotSet, ::fundle::NotSet, ::fundle::NotSet, ::fundle::NotSet> { fn default() -> Self { Self { logger1: ::std::option::Option::None, logger2: ::std::option::Option::None, config: ::std::option::Option::None, telemetry: ::std::option::Option::None, gpu: ::std::option::Option::None, _phantom: ::std::marker::PhantomData } } }
#[allow(non_camel_case_types, non_snake_case)]
impl<LOGGER2, CONFIG, TELEMETRY, GPU> AppStateBuilder<::fundle::NotSet, LOGGER2, CONFIG, TELEMETRY, GPU> {
    pub fn logger1(self, f: impl ::std::ops::Fn(&Self) -> Logger) -> AppStateBuilder<::fundle::Set, LOGGER2, CONFIG, TELEMETRY, GPU> {
        let logger1 = f(&self);
        AppStateBuilder {
            logger1: ::std::option::Option::Some(logger1),
            logger2: self.logger2,
            config: self.config,
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        }
    }
    pub fn logger1_try<R: ::std::error::Error>(self, f: impl ::std::ops::Fn(&Self) -> ::std::result::Result<Logger,
        R>) -> ::std::result::Result<AppStateBuilder<::fundle::Set, LOGGER2, CONFIG, TELEMETRY, GPU>, R> {
        let logger1 = f(&self)?;
        ::std::result::Result::Ok(AppStateBuilder {
            logger1: ::std::option::Option::Some(logger1),
            logger2: self.logger2,
            config: self.config,
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        })
    }
    pub async fn logger1_try_async<F, R: ::std::error::Error>(self, f: F) -> ::std::result::Result<AppStateBuilder<::fundle::Set, LOGGER2, CONFIG, TELEMETRY, GPU>, R>
    where
        F: AsyncFn(&Self) -> ::std::result::Result<Logger,
            R>,
    {
        let logger1 = f(&self).await?;
        ::std::result::Result::Ok(AppStateBuilder {
            logger1: ::std::option::Option::Some(logger1),
            logger2: self.logger2,
            config: self.config,
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        })
    }
    pub async fn logger1_async<F>(self, f: F) -> AppStateBuilder<::fundle::Set, LOGGER2, CONFIG, TELEMETRY, GPU>
    where
        F: AsyncFn(&Self) -> Logger,
    {
        let logger1 = f(&self).await;
        AppStateBuilder {
            logger1: ::std::option::Option::Some(logger1),
            logger2: self.logger2,
            config: self.config,
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        }
    }
}

#[allow(non_camel_case_types, non_snake_case)]
impl<LOGGER1, CONFIG, TELEMETRY, GPU> AppStateBuilder<LOGGER1, ::fundle::Set, CONFIG, TELEMETRY, GPU> {
    pub fn logger2(&self) ->  &Logger {
        self.logger2.as_ref().unwrap()
    }
}

#[allow(non_camel_case_types, non_snake_case)]
impl<LOGGER1, CONFIG, TELEMETRY, GPU> AppStateBuilder<LOGGER1, ::fundle::NotSet, CONFIG, TELEMETRY, GPU> {
    pub fn logger2(self, f: impl ::std::ops::Fn(&Self) -> Logger) -> AppStateBuilder<LOGGER1, ::fundle::Set, CONFIG, TELEMETRY, GPU> {
        let logger2 = f(&self);
        AppStateBuilder {
            logger1: self.logger1,
            logger2: ::std::option::Option::Some(logger2),
            config: self.config,
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        }
    }
    pub fn logger2_try<R: ::std::error::Error>(self, f: impl ::std::ops::Fn(&Self) -> ::std::result::Result<Logger,
        R>) -> ::std::result::Result<AppStateBuilder<LOGGER1, ::fundle::Set, CONFIG, TELEMETRY, GPU>, R> {
        let logger2 = f(&self)?;
        ::std::result::Result::Ok(AppStateBuilder {
            logger1: self.logger1,
            logger2: ::std::option::Option::Some(logger2),
            config: self.config,
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        })
    }
    pub async fn logger2_try_async<F, R: ::std::error::Error>(self, f: F) -> ::std::result::Result<AppStateBuilder<LOGGER1, ::fundle::Set, CONFIG, TELEMETRY, GPU>, R>
    where
        F: AsyncFn(&Self) -> ::std::result::Result<Logger,
            R>,
    {
        let logger2 = f(&self).await?;
        ::std::result::Result::Ok(AppStateBuilder {
            logger1: self.logger1,
            logger2: ::std::option::Option::Some(logger2),
            config: self.config,
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        })
    }
    pub async fn logger2_async<F>(self, f: F) -> AppStateBuilder<LOGGER1, ::fundle::Set, CONFIG, TELEMETRY, GPU>
    where
        F: AsyncFn(&Self) -> Logger,
    {
        let logger2 = f(&self).await;
        AppStateBuilder {
            logger1: self.logger1,
            logger2: ::std::option::Option::Some(logger2),
            config: self.config,
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        }
    }
}
#[allow(non_camel_case_types, non_snake_case)]
impl<LOGGER1, LOGGER2, TELEMETRY, GPU> AppStateBuilder<LOGGER1, LOGGER2, ::fundle::NotSet, TELEMETRY, GPU> {
    pub fn config(self, f: impl ::std::ops::Fn(&Self) -> Config) -> AppStateBuilder<LOGGER1, LOGGER2, ::fundle::Set, TELEMETRY, GPU> {
        let config = f(&self);
        AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: ::std::option::Option::Some(config),
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        }
    }
    pub fn config_try<R: ::std::error::Error>(self, f: impl ::std::ops::Fn(&Self) -> ::std::result::Result<Config,
        R>) -> ::std::result::Result<AppStateBuilder<LOGGER1, LOGGER2, ::fundle::Set, TELEMETRY, GPU>, R> {
        let config = f(&self)?;
        ::std::result::Result::Ok(AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: ::std::option::Option::Some(config),
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        })
    }
    pub async fn config_try_async<F, R: ::std::error::Error>(self, f: F) -> ::std::result::Result<AppStateBuilder<LOGGER1, LOGGER2, ::fundle::Set, TELEMETRY, GPU>, R>
    where
        F: AsyncFn(&Self) -> ::std::result::Result<Config,
            R>,
    {
        let config = f(&self).await?;
        ::std::result::Result::Ok(AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: ::std::option::Option::Some(config),
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        })
    }
    pub async fn config_async<F>(self, f: F) -> AppStateBuilder<LOGGER1, LOGGER2, ::fundle::Set, TELEMETRY, GPU>
    where
        F: AsyncFn(&Self) -> Config,
    {
        let config = f(&self).await;
        AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: ::std::option::Option::Some(config),
            telemetry: self.telemetry,
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        }
    }
}
#[allow(non_camel_case_types, non_snake_case)]
impl<LOGGER1, LOGGER2, CONFIG, GPU> AppStateBuilder<LOGGER1, LOGGER2, CONFIG, ::fundle::NotSet, GPU> {
    pub fn telemetry(self, f: impl ::std::ops::Fn(&Self) -> Telemetry) -> AppStateBuilder<LOGGER1, LOGGER2, CONFIG, ::fundle::Set, GPU> {
        let telemetry = f(&self);
        AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: self.config,
            telemetry: ::std::option::Option::Some(telemetry),
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        }
    }
    pub fn telemetry_try<R: ::std::error::Error>(self, f: impl ::std::ops::Fn(&Self) -> ::std::result::Result<Telemetry,
        R>) -> ::std::result::Result<AppStateBuilder<LOGGER1, LOGGER2, CONFIG, ::fundle::Set, GPU>, R> {
        let telemetry = f(&self)?;
        ::std::result::Result::Ok(AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: self.config,
            telemetry: ::std::option::Option::Some(telemetry),
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        })
    }
    pub async fn telemetry_try_async<F, R: ::std::error::Error>(self, f: F) -> ::std::result::Result<AppStateBuilder<LOGGER1, LOGGER2, CONFIG, ::fundle::Set, GPU>, R>
    where
        F: AsyncFn(&Self) -> ::std::result::Result<Telemetry,
            R>,
    {
        let telemetry = f(&self).await?;
        ::std::result::Result::Ok(AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: self.config,
            telemetry: ::std::option::Option::Some(telemetry),
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        })
    }
    pub async fn telemetry_async<F>(self, f: F) -> AppStateBuilder<LOGGER1, LOGGER2, CONFIG, ::fundle::Set, GPU>
    where
        F: AsyncFn(&Self) -> Telemetry,
    {
        let telemetry = f(&self).await;
        AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: self.config,
            telemetry: ::std::option::Option::Some(telemetry),
            gpu: self.gpu,
            _phantom: ::std::marker::PhantomData,
        }
    }
}
#[allow(non_camel_case_types, non_snake_case)]
impl<LOGGER1, LOGGER2, CONFIG, TELEMETRY> AppStateBuilder<LOGGER1, LOGGER2, CONFIG, TELEMETRY, ::fundle::NotSet> {
    pub fn gpu(self, f: impl ::std::ops::Fn(&Self) -> gpu::GpuBundle) -> AppStateBuilder<LOGGER1, LOGGER2, CONFIG, TELEMETRY, ::fundle::Set> {
        let gpu = f(&self);
        AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: self.config,
            telemetry: self.telemetry,
            gpu: ::std::option::Option::Some(gpu),
            _phantom: ::std::marker::PhantomData,
        }
    }
    pub fn gpu_try<R: ::std::error::Error>(self, f: impl ::std::ops::Fn(&Self) -> ::std::result::Result<gpu::GpuBundle,
        R>) -> ::std::result::Result<AppStateBuilder<LOGGER1, LOGGER2, CONFIG, TELEMETRY, ::fundle::Set>, R> {
        let gpu = f(&self)?;
        ::std::result::Result::Ok(AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: self.config,
            telemetry: self.telemetry,
            gpu: ::std::option::Option::Some(gpu),
            _phantom: ::std::marker::PhantomData,
        })
    }
    pub async fn gpu_try_async<F, R: ::std::error::Error>(self, f: F) -> ::std::result::Result<AppStateBuilder<LOGGER1, LOGGER2, CONFIG, TELEMETRY, ::fundle::Set>, R>
    where
        F: AsyncFn(&Self) -> ::std::result::Result<gpu::GpuBundle,
            R>,
    {
        let gpu = f(&self).await?;
        ::std::result::Result::Ok(AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: self.config,
            telemetry: self.telemetry,
            gpu: ::std::option::Option::Some(gpu),
            _phantom: ::std::marker::PhantomData,
        })
    }
    pub async fn gpu_async<F>(self, f: F) -> AppStateBuilder<LOGGER1, LOGGER2, CONFIG, TELEMETRY, ::fundle::Set>
    where
        F: AsyncFn(&Self) -> gpu::GpuBundle,
    {
        let gpu = f(&self).await;
        AppStateBuilder {
            logger1: self.logger1,
            logger2: self.logger2,
            config: self.config,
            telemetry: self.telemetry,
            gpu: ::std::option::Option::Some(gpu),
            _phantom: ::std::marker::PhantomData,
        }
    }
}
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl<LOGGER1, LOGGER2, TELEMETRY, GPU> ::std::convert::AsRef<Config> for AppStateBuilder<LOGGER1, LOGGER2, ::fundle::Set, TELEMETRY, GPU> { fn as_ref(&self) -> &Config { self.config.as_ref().unwrap() } }
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl<LOGGER1, LOGGER2, CONFIG, GPU> ::std::convert::AsRef<Telemetry> for AppStateBuilder<LOGGER1, LOGGER2, CONFIG, ::fundle::Set, GPU> { fn as_ref(&self) -> &Telemetry { self.telemetry.as_ref().unwrap() } }
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl<LOGGER1, LOGGER2, CONFIG, TELEMETRY> ::std::convert::AsRef<gpu::GpuBundle> for AppStateBuilder<LOGGER1, LOGGER2, CONFIG, TELEMETRY, ::fundle::Set> { fn as_ref(&self) -> &gpu::GpuBundle { self.gpu.as_ref().unwrap() } }
#[allow(non_camel_case_types, non_snake_case)]
impl ::std::convert::AsRef<gpu::Instance> for AppState {
    fn as_ref(&self) -> &gpu::Instance { self.gpu.as_ref() }
}
#[allow(non_camel_case_types, non_snake_case)]
impl ::std::convert::AsRef<gpu::Device> for AppState {
    fn as_ref(&self) -> &gpu::Device { self.gpu.as_ref() }
}
#[allow(non_camel_case_types, non_snake_case)]
impl ::std::convert::AsRef<gpu::Vulkan> for AppState {
    fn as_ref(&self) -> &gpu::Vulkan { self.gpu.as_ref() }
}
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl<LOGGER1, LOGGER2, CONFIG, TELEMETRY> ::std::convert::AsRef<gpu::Instance> for AppStateBuilder<LOGGER1, LOGGER2, CONFIG, TELEMETRY, ::fundle::Set> { fn as_ref(&self) -> &gpu::Instance { self.gpu.as_ref().unwrap().as_ref() } }
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl<LOGGER1, LOGGER2, CONFIG, TELEMETRY> ::std::convert::AsRef<gpu::Device> for AppStateBuilder<LOGGER1, LOGGER2, CONFIG, TELEMETRY, ::fundle::Set> { fn as_ref(&self) -> &gpu::Device { self.gpu.as_ref().unwrap().as_ref() } }
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl<LOGGER1, LOGGER2, CONFIG, TELEMETRY> ::std::convert::AsRef<gpu::Vulkan> for AppStateBuilder<LOGGER1, LOGGER2, CONFIG, TELEMETRY, ::fundle::Set> { fn as_ref(&self) -> &gpu::Vulkan { self.gpu.as_ref().unwrap().as_ref() } }
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl ::std::convert::AsRef<Config> for AppState {
    fn as_ref(&self) -> &Config { &self.config }
}
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl ::std::convert::AsRef<Telemetry> for AppState {
    fn as_ref(&self) -> &Telemetry { &self.telemetry }
}
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl ::std::convert::AsRef<gpu::GpuBundle> for AppState {
    fn as_ref(&self) -> &gpu::GpuBundle { &self.gpu }
}
impl ::fundle::exports::Exports for AppState {
    const NUM_EXPORTS: usize = 5usize;
}
#[allow(clippy::items_after_statements)]
impl ::fundle::exports::Export<0usize> for AppState {
    type T = Logger;
    fn get(&self) -> &Self::T { &self.logger1 }
}
#[allow(clippy::items_after_statements)]
impl ::fundle::exports::Export<1usize> for AppState {
    type T = Logger;
    fn get(&self) -> &Self::T { &self.logger2 }
}
#[allow(clippy::items_after_statements)]
impl ::fundle::exports::Export<2usize> for AppState {
    type T = Config;
    fn get(&self) -> &Self::T { &self.config }
}
#[allow(clippy::items_after_statements)]
impl ::fundle::exports::Export<3usize> for AppState {
    type T = Telemetry;
    fn get(&self) -> &Self::T { &self.telemetry }
}
#[allow(clippy::items_after_statements)]
impl ::fundle::exports::Export<4usize> for AppState {
    type T = gpu::GpuBundle;
    fn get(&self) -> &Self::T { &self.gpu }
}
#[allow(non_camel_case_types, non_snake_case)]
impl<LOGGER2, CONFIG, TELEMETRY, GPU> ::fundle::exports::Export<0usize> for AppStateBuilder<::fundle::Set, LOGGER2, CONFIG, TELEMETRY, GPU> {
    type T = Logger;
    fn get(&self) -> &Self::T { self.logger1.as_ref().unwrap() }
}
#[allow(non_camel_case_types, non_snake_case)]
impl<LOGGER1, CONFIG, TELEMETRY, GPU> ::fundle::exports::Export<1usize> for AppStateBuilder<LOGGER1, ::fundle::Set, CONFIG, TELEMETRY, GPU> {
    type T = Logger;
    fn get(&self) -> &Self::T { self.logger2.as_ref().unwrap() }
}
#[allow(non_camel_case_types, non_snake_case)]
impl<LOGGER1, LOGGER2, TELEMETRY, GPU> ::fundle::exports::Export<2usize> for AppStateBuilder<LOGGER1, LOGGER2, ::fundle::Set, TELEMETRY, GPU> {
    type T = Config;
    fn get(&self) -> &Self::T { self.config.as_ref().unwrap() }
}
#[allow(non_camel_case_types, non_snake_case)]
impl<LOGGER1, LOGGER2, CONFIG, GPU> ::fundle::exports::Export<3usize> for AppStateBuilder<LOGGER1, LOGGER2, CONFIG, ::fundle::Set, GPU> {
    type T = Telemetry;
    fn get(&self) -> &Self::T { self.telemetry.as_ref().unwrap() }
}
#[allow(non_camel_case_types, non_snake_case)]
impl<LOGGER1, LOGGER2, CONFIG, TELEMETRY> ::fundle::exports::Export<4usize> for AppStateBuilder<LOGGER1, LOGGER2, CONFIG, TELEMETRY, ::fundle::Set> {
    type T = gpu::GpuBundle;
    fn get(&self) -> &Self::T { self.gpu.as_ref().unwrap() }
}
#[allow(non_camel_case_types, non_snake_case, clippy::items_after_statements)]
impl AppStateBuilder<::fundle::Set, ::fundle::Set, ::fundle::Set, ::fundle::Set, ::fundle::Set> {
    pub fn build(self) -> AppState {
        AppState {
            logger1: self.logger1.unwrap(),
            logger2: self.logger2.unwrap(),
            config: self.config.unwrap(),
            telemetry: self.telemetry.unwrap(),
            gpu: self.gpu.unwrap(),
        }
    }
}
#[allow(unused_macros, snake_case)] macro_rules! AppState {
     ( verify_field   $   builder_var   :   ident   logger1 ) =>   { { fn   verify_exists   <   T2   ,   T3   ,   T4   ,   T5   >   ( _   :   &   AppStateBuilder   <   ::   fundle   ::   Set   ,   T2   ,   T3   ,   T4   ,   T5   >   ) {  } verify_exists   ( $   builder_var   ) ;   } } ;   ( verify_field   $   builder_var   :   ident   logger2 ) =>   { { fn   verify_exists   <   T1   ,   T3   ,   T4   ,   T5   >   ( _   :   &   AppStateBuilder   <   T1   ,   ::   fundle   ::   Set   ,   T3   ,   T4   ,   T5   >   ) {  } verify_exists   ( $   builder_var   ) ;   } } ;   ( verify_field   $   builder_var   :   ident   config ) =>   { { fn   verify_exists   <   T1   ,   T2   ,   T4   ,   T5   >   ( _   :   &   AppStateBuilder   <   T1   ,   T2   ,   ::   fundle   ::   Set   ,   T4   ,   T5   >   ) {  } verify_exists   ( $   builder_var   ) ;   } } ;   ( verify_field   $   builder_var   :   ident   telemetry ) =>   { { fn   verify_exists   <   T1   ,   T2   ,   T3   ,   T5   >   ( _   :   &   AppStateBuilder   <   T1   ,   T2   ,   T3   ,   ::   fundle   ::   Set   ,   T5   >   ) {  } verify_exists   ( $   builder_var   ) ;   } } ;   ( verify_field   $   builder_var   :   ident   gpu ) =>   { { fn   verify_exists   <   T1   ,   T2   ,   T3   ,   T4   >   ( _   :   &   AppStateBuilder   <   T1   ,   T2   ,   T3   ,   T4   ,   ::   fundle   ::   Set   >   ) {  } verify_exists   ( $   builder_var   ) ;   } } ;   ( select   ( $   builder_var   :   ident   ) =>   $   ( $   forward_type   :   ident   ( $   forward_field   :   ident   ) ) ,   *   $   ( ,   ) ?   ) =>   { { $   ( AppState   !   ( verify_field   $   builder_var   $   forward_field   ) ;   ) *   #   [ allow   ( non_camel_case_types   ,   non_snake_case   ,   clippy   ::   items_after_statements   ) ] struct   Select   <   'a   ,   T1   ,   T2   ,   T3   ,   T4   ,   T5   >   { builder   :   &   'a   AppStateBuilder   <   T1   ,   T2   ,   T3   ,   T4   ,   T5   >   ,   $   ( $   forward_type   :   &   'a   $   forward_type   ,   ) *   } impl   <   'a   ,   T1   ,   T2   ,   T4   ,   T5   >   ::   std   ::   convert   ::   AsRef   <   Config   >   for   Select   <   'a   ,   T1   ,   T2   ,   ::   fundle   ::   Set   ,   T4   ,   T5   >   where   AppStateBuilder   <   T1   ,   T2   ,   ::   fundle   ::   Set   ,   T4   ,   T5   >   :   ::   std   ::   convert   ::   AsRef   <   Config   >   ,   { fn   as_ref   ( &   self   ) ->   &   Config { self   .   builder   .   as_ref   (  ) } } impl   <   'a   ,   T1   ,   T2   ,   T3   ,   T5   >   ::   std   ::   convert   ::   AsRef   <   Telemetry   >   for   Select   <   'a   ,   T1   ,   T2   ,   T3   ,   ::   fundle   ::   Set   ,   T5   >   where   AppStateBuilder   <   T1   ,   T2   ,   T3   ,   ::   fundle   ::   Set   ,   T5   >   :   ::   std   ::   convert   ::   AsRef   <   Telemetry   >   ,   { fn   as_ref   ( &   self   ) ->   &   Telemetry { self   .   builder   .   as_ref   (  ) } } impl   <   'a   ,   T1   ,   T2   ,   T3   ,   T4   >   ::   std   ::   convert   ::   AsRef   <   gpu :: GpuBundle   >   for   Select   <   'a   ,   T1   ,   T2   ,   T3   ,   T4   ,   ::   fundle   ::   Set   >   where   AppStateBuilder   <   T1   ,   T2   ,   T3   ,   T4   ,   ::   fundle   ::   Set   >   :   ::   std   ::   convert   ::   AsRef   <   gpu :: GpuBundle   >   ,   { fn   as_ref   ( &   self   ) ->   &   gpu :: GpuBundle { self   .   builder   .   as_ref   (  ) } } $   ( #   [ allow   ( non_camel_case_types   ,   non_snake_case   ,   clippy   ::   items_after_statements   ) ] impl   <   'a   ,   T1   ,   T2   ,   T3   ,   T4   ,   T5   >   ::   std   ::   convert   ::   AsRef   <   $   forward_type   >   for   Select   <   'a   ,   T1   ,   T2   ,   T3   ,   T4   ,   T5   >   { fn   as_ref   ( &   self   ) ->   &   $   forward_type   { self   .   $   forward_type   } } ) *   Select   { builder   :   &   $   builder_var   ,   $   ( $   forward_type   :   $   builder_var   .   $   forward_field   .   as_ref   (  ) .   unwrap   (  ) ,   ) *   } } } ;   }