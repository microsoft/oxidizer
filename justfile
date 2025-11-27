# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
#
# https://just.systems

set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
set shell := ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
#set script-interpreter := ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive"]


mutants: 
    scripts/mutants.ps1

test:
    cargo test --all --all-features --locked
