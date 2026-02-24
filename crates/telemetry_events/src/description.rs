use std::borrow::Cow;

#[derive(Clone, Copy)]
pub struct EventDescription {
    pub name: &'static str,
    pub id: u64,
    pub fields: &'static [FieldDescription],
}

#[derive(Clone, Copy)]
pub struct FieldDescription {
    pub name: &'static str,
    pub index: u64,
}
