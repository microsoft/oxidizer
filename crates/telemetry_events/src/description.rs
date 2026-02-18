use std::borrow::Cow;

pub struct EventDescription {
    pub name: &'static str,
    pub id: u64,
    pub fields: &'static [FieldDescription],
}

pub struct FieldDescription {
    pub name: &'static str,
    pub index: u64,
}
