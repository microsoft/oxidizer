const _: () = {
    use ::multitude::de::__private::serde as __Serde;
    struct __MultitudeVisitorForUnit<'__arena, __A>
    where
        __A: ::multitude::de::__private::allocator_api2::alloc::Allocator + ::core::clone::Clone,
    {
        arena: &'__arena ::multitude::Arena<__A>,
        marker: ::core::marker::PhantomData<fn() -> Unit>,
    }
    impl<'de, '__arena, __A> __Serde::de::Visitor<'de> for __MultitudeVisitorForUnit<'__arena, __A>
    where
        __A: ::multitude::de::__private::allocator_api2::alloc::Allocator + ::core::clone::Clone,
    {
        type Value = Unit;
        fn expecting(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            formatter.write_str("a unit value")
        }
        fn visit_unit<__E>(self) -> ::core::result::Result<Self::Value, __E>
        where
            __E: __Serde::de::Error,
        {
            ::core::result::Result::Ok(Unit)
        }
    }
    impl<'de, __A> ::multitude::de::DeserializeIn<'de, __A> for Unit
    where
        __A: ::multitude::de::__private::allocator_api2::alloc::Allocator + ::core::clone::Clone,
    {
        fn deserialize_in<__D>(arena: &::multitude::Arena<__A>, deserializer: __D) -> ::core::result::Result<Self, __D::Error>
        where
            __D: __Serde::Deserializer<'de>,
        {
            __Serde::Deserializer::deserialize_unit_struct(
                deserializer,
                "Unit",
                __MultitudeVisitorForUnit {
                    arena,
                    marker: ::core::marker::PhantomData,
                },
            )
        }
    }
};
