// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prepared, memory-backed response templates.

use core::marker::PhantomData;

use bytesbuf::BytesView;
use bytesbuf::mem::Memory;

use crate::response::__template::{self, HtmlText, Integer, JsonNumber, JsonString, PlainText, Slot};

/// The escaping discipline a prepared template applies to every dynamic slot.
///
/// A template's kind is chosen once, when the fixed fragments are prepared, and
/// [`BytesViewTemplate::render`] then accepts only slots belonging to that kind.
/// Placing verbatim text inside a JSON or HTML template is therefore a
/// compile-time error rather than a review question.
pub trait TemplateKind: sealed::TemplateKind {}

/// A template whose fixed fragments are JSON.
///
/// Accepts [`json_number`] and [`json_string`] slots only.
#[derive(Clone, Copy, Debug)]
pub struct Json;

/// A template whose fixed fragments are HTML.
///
/// Accepts [`html_text`] slots only.
#[derive(Clone, Copy, Debug)]
pub struct Html;

/// A template whose fixed fragments are `text/plain`.
///
/// Accepts [`unescaped_text`] slots, which is safe precisely because no
/// surrounding markup or JSON structure can be terminated by their content.
#[derive(Clone, Copy, Debug)]
pub struct Plain;

impl sealed::TemplateKind for Json {}
impl TemplateKind for Json {}
impl sealed::TemplateKind for Html {}
impl TemplateKind for Html {}
impl sealed::TemplateKind for Plain {}
impl TemplateKind for Plain {}

/// A slot that may appear in a template of kind `K`.
///
/// The escaping a slot performs and the kind it belongs to are one decision, so
/// there is no way to add an unescaped slot to an escaped kind.
pub trait SlotFor<K: TemplateKind>: Slot {}

impl<T: Integer> SlotFor<Json> for JsonNumber<T> {}
impl<T: AsRef<str>> SlotFor<Json> for JsonString<T> {}
impl<T: AsRef<str>> SlotFor<Html> for HtmlText<T> {}
impl<T: AsRef<str>> SlotFor<Plain> for PlainText<T> {}

/// Prepared fixed fragments for a typed `BytesView` response template.
///
/// Preparation copies each fixed fragment once into memory selected by the
/// application. Rendering clones those views and writes only dynamic slots.
///
/// `K` fixes the escaping for the whole template; see [`TemplateKind`].
#[derive(Clone, Debug)]
pub struct BytesViewTemplate<const N: usize, K: TemplateKind = Plain> {
    fragments: [BytesView; N],
    kind: PhantomData<fn() -> K>,
}

impl<const N: usize, K: TemplateKind> BytesViewTemplate<N, K> {
    /// Copies fixed fragments into reusable views.
    ///
    /// Prefer [`json_view_template!`], [`html_view_template!`], or
    /// [`text_view_template!`], which name the kind at the point of use.
    ///
    /// [`json_view_template!`]: crate::response::json_view_template
    /// [`html_view_template!`]: crate::response::html_view_template
    /// [`text_view_template!`]: crate::response::text_view_template
    #[must_use]
    pub fn prepare<M>(memory: &M, fragments: [&[u8]; N]) -> Self
    where
        M: Memory,
    {
        Self {
            fragments: fragments.map(|fragment| BytesView::copied_from_slice(fragment, memory)),
            kind: PhantomData,
        }
    }

    /// Renders typed slots between the prepared fixed fragments.
    ///
    /// A template with `N` fragments accepts `N - 1` slots. The supported slot
    /// tuple arities are zero through eight. Every slot must belong to this
    /// template's kind.
    #[must_use]
    pub fn render<M, S>(&self, memory: &M, slots: S) -> BytesView
    where
        M: Memory,
        S: TemplateSlots<N, K>,
    {
        slots.render(self, memory)
    }

    /// Returns the prepared fixed fragments.
    #[must_use]
    pub const fn fragments(&self) -> &[BytesView; N] {
        &self.fragments
    }
}

/// Creates a JSON integer slot.
#[must_use]
pub fn json_number<T: Integer>(value: T) -> JsonNumber<T> {
    __template::json_number(value)
}

/// Creates an escaped JSON string slot, including its quotes.
#[must_use]
pub fn json_string<T: AsRef<str>>(value: T) -> JsonString<T> {
    __template::json_string(value)
}

/// Creates an escaped HTML text-content slot.
///
/// The five HTML-sensitive characters are escaped, so the value cannot close
/// the surrounding element or introduce markup.
#[must_use]
pub fn html_text<T: AsRef<str>>(value: T) -> HtmlText<T> {
    __template::html_text(value)
}

/// Creates a plain-text slot that is written verbatim.
///
/// Nothing is escaped, which is why this slot belongs to the [`Plain`] kind
/// alone: a [`Json`] or [`Html`] template will not accept it. Use
/// [`json_string`] inside JSON templates and [`html_text`] inside HTML ones.
#[must_use]
pub fn unescaped_text<T: AsRef<str>>(value: T) -> PlainText<T> {
    __template::plain_text(value)
}

mod sealed {
    pub trait TemplateSlots<const N: usize, K> {}
    pub trait TemplateKind {}
}

/// A typed slot tuple accepted by [`BytesViewTemplate::render`].
#[doc(hidden)]
pub trait TemplateSlots<const N: usize, K: TemplateKind>: sealed::TemplateSlots<N, K> {
    fn render<M>(self, template: &BytesViewTemplate<N, K>, memory: &M) -> BytesView
    where
        M: Memory;
}

impl<K: TemplateKind> sealed::TemplateSlots<1, K> for () {}

impl<K: TemplateKind> TemplateSlots<1, K> for () {
    fn render<M>(self, template: &BytesViewTemplate<1, K>, _memory: &M) -> BytesView
    where
        M: Memory,
    {
        template.fragments[0].clone()
    }
}

macro_rules! template_slots {
    ($fragments:literal; $(($slot_type:ident, $slot:ident, $fragment:tt)),+ $(,)?) => {
        impl<K: TemplateKind, $($slot_type),+> sealed::TemplateSlots<$fragments, K> for ($($slot_type,)+)
        where
            $($slot_type: SlotFor<K>),+
        {
        }

        impl<K: TemplateKind, $($slot_type),+> TemplateSlots<$fragments, K> for ($($slot_type,)+)
        where
            $($slot_type: SlotFor<K>),+
        {
            fn render<M>(self, template: &BytesViewTemplate<$fragments, K>, memory: &M) -> BytesView
            where
                M: Memory,
            {
                let ($($slot,)+) = self;
                let dynamic_length = 0_usize
                    $(.checked_add($slot.encoded_len())
                        .expect("encoded template slots must fit in usize"))+;
                let mut output = memory.reserve(dynamic_length);
                output.put_bytes(template.fragments[0].clone());
                $(
                    $slot.write_to(&mut output);
                    output.put_bytes(template.fragments[$fragment].clone());
                )+
                output.consume_all()
            }
        }
    };
}

template_slots!(2; (S0, slot0, 1));
template_slots!(3; (S0, slot0, 1), (S1, slot1, 2));
template_slots!(4; (S0, slot0, 1), (S1, slot1, 2), (S2, slot2, 3));
template_slots!(
    5;
    (S0, slot0, 1),
    (S1, slot1, 2),
    (S2, slot2, 3),
    (S3, slot3, 4)
);
template_slots!(
    6;
    (S0, slot0, 1),
    (S1, slot1, 2),
    (S2, slot2, 3),
    (S3, slot3, 4),
    (S4, slot4, 5)
);
template_slots!(
    7;
    (S0, slot0, 1),
    (S1, slot1, 2),
    (S2, slot2, 3),
    (S3, slot3, 4),
    (S4, slot4, 5),
    (S5, slot5, 6)
);
template_slots!(
    8;
    (S0, slot0, 1),
    (S1, slot1, 2),
    (S2, slot2, 3),
    (S3, slot3, 4),
    (S4, slot4, 5),
    (S5, slot5, 6),
    (S6, slot6, 7)
);
template_slots!(
    9;
    (S0, slot0, 1),
    (S1, slot1, 2),
    (S2, slot2, 3),
    (S3, slot3, 4),
    (S4, slot4, 5),
    (S5, slot5, 6),
    (S6, slot6, 7),
    (S7, slot7, 8)
);

/// Prepares a JSON [`BytesViewTemplate`] from its fixed fragments.
///
/// The template's kind is [`Json`], so rendering accepts [`json_number`] and
/// [`json_string`] slots only; [`unescaped_text`] will not compile against it.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(all(feature = "response", feature = "bytesbuf-std"))] {
/// use bytesbuf::mem::GlobalPool;
/// use routerama::response::bytesbuf::template::{json_number, json_string};
/// use routerama::response::json_view_template;
///
/// let memory = GlobalPool::new();
/// let template = json_view_template!(&memory, br#"{"id":"#, br#","name":"#, b"}");
/// let body = template.render(&memory, (json_number(7_u32), json_string("ferris")));
///
/// assert_eq!(body.to_vec(), br#"{"id":7,"name":"ferris"}"#);
/// # }
/// # }
/// ```
#[macro_export]
macro_rules! __routerama_json_view_template {
    ($memory:expr, $($fragment:expr),+ $(,)?) => {
        $crate::response::bytesbuf::template::BytesViewTemplate::<
            { $crate::response::bytesbuf::template::__count!($($fragment),+) },
            $crate::response::bytesbuf::template::Json,
        >::prepare($memory, [$($fragment.as_slice()),+])
    };
}

/// Prepares an HTML [`BytesViewTemplate`] from its fixed fragments.
///
/// The template's kind is [`Html`], so rendering accepts [`html_text`] slots
/// only; [`unescaped_text`] will not compile against it, which is what stops
/// unescaped markup reaching an HTML response.
#[macro_export]
macro_rules! __routerama_html_view_template {
    ($memory:expr, $($fragment:expr),+ $(,)?) => {
        $crate::response::bytesbuf::template::BytesViewTemplate::<
            { $crate::response::bytesbuf::template::__count!($($fragment),+) },
            $crate::response::bytesbuf::template::Html,
        >::prepare($memory, [$($fragment.as_slice()),+])
    };
}

/// Prepares a `text/plain` [`BytesViewTemplate`] from its fixed fragments.
///
/// The template's kind is [`Plain`], the one kind for which [`unescaped_text`]
/// is safe, because plain text has no structure a value could terminate.
#[macro_export]
macro_rules! __routerama_text_view_template {
    ($memory:expr, $($fragment:expr),+ $(,)?) => {
        $crate::response::bytesbuf::template::BytesViewTemplate::<
            { $crate::response::bytesbuf::template::__count!($($fragment),+) },
            $crate::response::bytesbuf::template::Plain,
        >::prepare($memory, [$($fragment.as_slice()),+])
    };
}

/// Counts the fragments passed to a view-template macro.
#[doc(hidden)]
#[macro_export]
macro_rules! __routerama_count_fragments {
    () => { 0_usize };
    ($head:expr $(, $tail:expr)*) => { 1_usize + $crate::response::bytesbuf::template::__count!($($tail),*) };
}

#[doc(hidden)]
pub use crate::__routerama_count_fragments as __count;
