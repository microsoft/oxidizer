# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

# Required by [script]
set unstable

set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
set script-interpreter := ["pwsh"]

# Constants shared by Just commands and GitHub workflows.
set dotenv-path := "./constants.env"
set dotenv-required := true

package := ""
target_package := if package == "" { "--workspace" } else { "-p " + package }

_default:
    @just --list

import 'justfiles/basic.just'
import 'justfiles/coverage.just'
import 'justfiles/format.just'
import 'justfiles/setup.just'
import 'justfiles/spelling.just'
