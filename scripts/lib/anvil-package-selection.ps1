# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

function ConvertTo-WorkspacePackageSelection {
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string]$Selection,
        [Parameter(Mandatory)][object]$Metadata,
        [ValidateSet('Cargo', 'MetadataId', 'NameVersion')][string]$Format = 'Cargo'
    )

    $specs = [System.Collections.Generic.Dictionary[string, string]]::new([System.StringComparer]::Ordinal)
    foreach ($package in $Metadata.packages) {
        if ($Metadata.workspace_members -ccontains $package.id) {
            $source = ([string]$package.id -split '#', 2)[0]
            $nameVersion = "$($package.name)@$($package.version)"
            $cargoSpec = "$source#$nameVersion"
            $outputSpec = switch ($Format) {
                'Cargo' { $cargoSpec }
                'MetadataId' { [string]$package.id }
                'NameVersion' { $nameVersion }
            }
            foreach ($alias in @([string]$package.name, $nameVersion, $cargoSpec, [string]$package.id) | Select-Object -Unique) {
                $specs.Add($alias, $outputSpec)
            }
        }
    }
    $tokens = @(-split $Selection | Where-Object { $_ })
    for ($i = 0; $i -lt $tokens.Count; $i++) {
        if ($tokens[$i] -in @('--workspace', '--skip')) { continue }
        if ($tokens[$i] -eq '--package') {
            $i++
            if ($i -ge $tokens.Count) { throw 'anvil: --package is missing its package spec' }
        }
        if (-not $specs.ContainsKey($tokens[$i])) {
            throw "anvil: unknown workspace package spec '$($tokens[$i])'"
        }
        $tokens[$i] = $specs[$tokens[$i]]
    }
    return $tokens -join ' '
}
