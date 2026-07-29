// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;

use crate::rt_node::RtNode;

/// An affix edge: a `prefix`, a `suffix`, and the subtree matched when a segment
/// both starts with the prefix and ends with the suffix.
pub(crate) type AffixEdge = (Box<str>, Box<str>, RtNode);
