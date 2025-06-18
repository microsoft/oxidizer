// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// If a lock is poisoned then safety invariants may have been violated and execution cannot
// continue because we can no longer uphold our security and privacy guarantees.
pub const ERR_POISONED_LOCK: &str = "poisoned lock - cannot continue execution because security and privacy guarantees can no longer be upheld";