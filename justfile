# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
set shell := ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
set script-interpreter := ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive"]

_default:
    @just --list

# >>> anvil-managed: anvil-imports
import 'justfiles/anvil/mod.just'
# <<< anvil-managed: anvil-imports

import 'justfiles/repository.just'
