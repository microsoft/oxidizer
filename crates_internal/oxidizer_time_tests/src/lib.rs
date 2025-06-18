// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(not(miri))] // Miri is not compatible with getting the real time from the OS.

mod time;