# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

$manifestDirectories = @($repoRoot)
$manifestDirectories += $metadata.packages |
    ForEach-Object { Split-Path -Parent $_.manifest_path } |
    Sort-Object -Unique

foreach ($directory in $manifestDirectories) {
    cargo sort --grouped --check --check-format $directory
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}
