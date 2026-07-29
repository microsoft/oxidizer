// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;

use crate::rt_node::RtNode;

/// A literal edge: an exact segment key and the subtree it leads to.
pub(crate) type LiteralEdge = (Box<str>, RtNode);
