# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $True

$jobs = 1
$build_timeout_sec = 600
$timeout_sec = 300
$minimum_test_timeout_sec = 60

cargo mutants `
    --no-shuffle `
    --baseline=skip `
    --test-workspace=true `
    --colors=never `
    --jobs=$jobs `
    --build-timeout=$build_timeout_sec `
    --timeout=$timeout_sec `
    --minimum-test-timeout=$minimum_test_timeout_sec `
    -vV

