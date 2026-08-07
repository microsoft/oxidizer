# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

$ErrorActionPreference = "Stop"

$metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

$packageDirectories = $metadata.packages |
    ForEach-Object { Split-Path -Parent $_.manifest_path } |
    Sort-Object -Unique

cargo machete --with-metadata $packageDirectories
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
