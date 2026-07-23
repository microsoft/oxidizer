// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prepared, memory-backed response templates.

use bytesbuf::BytesView;
use bytesbuf::mem::Memory;

use crate::response::__template::{self, Integer, Slot};

/// Prepared fixed fragments for a typed `BytesView` response template.
///
/// Preparation copies each fixed fragment once into memory selected by the
/// application. Rendering clones those views and writes only dynamic slots.
#[derive(Clone, Debug)]
pub struct BytesViewTemplate<const N: usize> {
    fragments: [BytesView; N],
}

impl<const N: usize> BytesViewTemplate<N> {
    /// Copies fixed fragments into reusable views.
    #[must_use]
    pub fn prepare<M>(memory: &M, fragments: [&[u8]; N]) -> Self
    where
        M: Memory,
    {
        Self {
            fragments: fragments.map(|fragment| BytesView::copied_from_slice(fragment, memory)),
        }
    }

    /// Renders typed slots between the prepared fixed fragments.
    ///
    /// A template with `N` fragments accepts `N - 1` slots. The supported slot
    /// tuple arities are zero through eight.
    #[must_use]
    pub fn render<M, S>(&self, memory: &M, slots: S) -> BytesView
    where
        M: Memory,
        S: TemplateSlots<N>,
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
pub fn json_number<T: Integer>(value: T) -> impl Slot {
    __template::json_number(value)
}

/// Creates an escaped JSON string slot, including its quotes.
#[must_use]
pub fn json_string<T: AsRef<str>>(value: T) -> impl Slot {
    __template::json_string(value)
}

/// Creates an escaped HTML text-content slot.
///
/// The five HTML-sensitive characters are escaped, so the value cannot close
/// the surrounding element or introduce markup.
#[must_use]
pub fn html_text<T: AsRef<str>>(value: T) -> impl Slot {
    __template::html_text(value)
}

/// Creates a plain-text slot that is written verbatim.
///
/// # Security
///
/// Nothing is escaped. This slot is only safe when the surrounding fragments
/// are `text/plain`, or when the caller has already validated that the value
/// cannot terminate the enclosing context. Use [`json_string`] inside JSON
/// fragments and [`html_text`] inside HTML fragments; the declarative
/// [`json_body_template!`] and [`html_body_template!`] macros deliberately
/// offer no verbatim slot at all.
///
/// [`json_body_template!`]: crate::response::json_body_template
/// [`html_body_template!`]: crate::response::html_body_template
#[must_use]
pub fn unescaped_text<T: AsRef<str>>(value: T) -> impl Slot {
    __template::plain_text(value)
}

mod sealed {
    pub trait TemplateSlots<const N: usize> {}
}

/// A typed slot tuple accepted by [`BytesViewTemplate::render`].
#[doc(hidden)]
pub trait TemplateSlots<const N: usize>: sealed::TemplateSlots<N> {
    fn render<M>(self, template: &BytesViewTemplate<N>, memory: &M) -> BytesView
    where
        M: Memory;
}

impl sealed::TemplateSlots<1> for () {}

impl TemplateSlots<1> for () {
    fn render<M>(self, template: &BytesViewTemplate<1>, _memory: &M) -> BytesView
    where
        M: Memory,
    {
        template.fragments[0].clone()
    }
}

macro_rules! template_slots {
    ($fragments:literal; $(($slot_type:ident, $slot:ident, $fragment:tt)),+ $(,)?) => {
        impl<$($slot_type),+> sealed::TemplateSlots<$fragments> for ($($slot_type,)+)
        where
            $($slot_type: Slot),+
        {
        }

        impl<$($slot_type),+> TemplateSlots<$fragments> for ($($slot_type,)+)
        where
            $($slot_type: Slot),+
        {
            fn render<M>(self, template: &BytesViewTemplate<$fragments>, memory: &M) -> BytesView
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
