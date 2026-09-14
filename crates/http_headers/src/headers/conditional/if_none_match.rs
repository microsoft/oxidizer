// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use super::shared::{
    ConditionalTagView, TagIter, TagListState, entity_tag_list_header, validate_tag_line_outlined_with, validate_tag_line_with,
    validate_tag_slice,
};
use crate::sink::{FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, Field, FieldName, FieldValue, FieldValueRef};

entity_tag_list_header!(
    IfNoneMatch,
    IfNoneMatchOwned,
    IfNoneMatchView,
    "If-None-Match",
    &FieldName::IfNoneMatch,
    "Owned value for the `If-None-Match` header.",
    "Borrowed value for the `If-None-Match` header.",
    "Defined by [RFC 9110 section 13.1.2](https://www.rfc-editor.org/rfc/rfc9110#section-13.1.2).",
    "`If-None-Match: *` matches any current representation. `If-None-Match: \"strong\", W/\"weak\"` demonstrates strong and weak tags."
);
