# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
set shell := ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
set script-interpreter := ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive"]

# Constants shared by Just commands and GitHub workflows.
set dotenv-path := "./constants.env"
set dotenv-required := true

package := ""
target_package := if package == "" { "--workspace" } else { "-p " + package }

_default:
    @just --list

import 'justfiles/basic.just'
import 'justfiles/coverage.just'
import 'justfiles/extended.just'
import 'justfiles/format.just'
import 'justfiles/setup.just'
import 'justfiles/spelling.just'

# >>> anvil-managed: anvil-imports
import 'justfiles/anvil/mod.just'
# <<< anvil-managed: anvil-imports
