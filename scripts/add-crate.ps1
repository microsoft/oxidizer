# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    This script is responsible for setting up a new crate in the repo.

.DESCRIPTION
    This script lets you bootstrap a new crate into this repo. It will:

    * Create a skeleton folder for the crate with:
      * Starter Cargo.toml
      * Starter README.md file with standard badges and support for cargo-rdme
      * Starter CHANGELOG.md file
    * Update the top-level README and CHANGELOG.md files to point to the new crate
    * Update the top-level Cargo.toml files to build the new crate

    When you run the script, it asks you for the crate name, description,
    list of keywords, and list of categories for the crate. Once it has
    collected this information, it adds a new fully configured crate to the
    repo.

.EXAMPLE
    .\scripts\add-crate.ps1
#>
param()

$ErrorActionPreference = "Stop"

# Get the script's directory
$scriptDir = $PSScriptRoot

# Get the repo root
$repoRoot = Resolve-Path -Path (Join-Path $scriptDir "..")

# Prompt for user input
$crateName = Read-Host -Prompt "Enter the crate name (e.g., my_awesome_crate)"
if ([string]::IsNullOrWhiteSpace($crateName)) {
    Write-Error "Crate name cannot be empty."
    exit 1
}

if ($crateName -match "-") {
    Write-Error "Crate name cannot contain dashes. Use underscores instead."
    exit 1
}

$crateDescription = Read-Host -Prompt "Enter the crate description"
$crateKeywords = Read-Host -Prompt "Enter comma-separated crate keywords (see https://crates.io/keywords for inspiration)"
$crateCategories = Read-Host -Prompt "Enter comma-separated crate categories (see https://crates.io/categories for allowed categories)"

# Define paths
$templateDir = Join-Path $repoRoot "scripts" "repo-template"
$destinationDir = Join-Path $repoRoot "crates" $crateName

# Check if crate already exists
if (Test-Path $destinationDir) {
    Write-Error "Crate '$crateName' already exists at '$destinationDir'."
    exit 1
}

# Copy template to destination
Write-Host "Copying template from '$templateDir' to '$destinationDir'..."
Copy-Item -Path $templateDir -Destination $destinationDir -Recurse

# Prepare replacement values
$crateNameUpper = ($crateName -replace '_', ' ').Split(' ') | ForEach-Object { $_.Substring(0, 1).ToUpper() + $_.Substring(1) } | Join-String -Separator ' '
$formattedKeywords = ($crateKeywords.Split(',') | ForEach-Object { "`"$($_.Trim())`"" }) -join ", "
$formattedCategories = ($crateCategories.Split(',') | ForEach-Object { "`"$($_.Trim())`"" }) -join ", "

# Perform substitutions
Get-ChildItem -Path $destinationDir -Recurse | ForEach-Object {
    if ($_.PSIsContainer) {
        return
    }

    $filePath = $_.FullName
    Write-Host "Processing $filePath..."
    $content = Get-Content $filePath -Raw

    $content = $content -replace '\{\{CRATE_NAME\}\}', $crateName
    $content = $content -replace '\{\{CRATE_NAME_UPPER\}\}', $crateNameUpper
    $content = $content -replace '\{\{CRATE_DESCRIPTION\}\}', $crateDescription
    $content = $content -replace '\{\{CRATE_KEYWORDS\}\}', $formattedKeywords
    $content = $content -replace '\{\{CRATE_CATEGORIES\}\}', $formattedCategories

    Set-Content -Path $filePath -Value $content -NoNewline
}

# Update root README.md
$readmePath = Join-Path $repoRoot "README.md"
$readmeLines = Get-Content $readmePath
$cratesList = @{}
$inCratesSection = $false
$readmeInsertionIndex = -1
$readmeEndIndex = -1

for ($i = 0; $i -lt $readmeLines.Length; $i++) {
    if ($readmeLines[$i] -eq "## Crates") {
        $inCratesSection = $true
        continue
    }
    if ($inCratesSection) {
        if ($readmeLines[$i] -match '^- \[`(.*)`\](.*)') {
            if($readmeInsertionIndex -eq -1) {
                $readmeInsertionIndex = $i
            }
            $cratesList[$Matches[1]] = $readmeLines[$i]
            $readmeEndIndex = $i
        } elseif ($readmeLines[$i].Trim() -ne "" -and $readmeInsertionIndex -ne -1) {
            break
        }
    }
}

$cratesList[$crateName] = ('- [`{0}`](./crates/{0}/README.md) - {1}' -f $crateName, $crateDescription)
$sortedCrateNames = $cratesList.Keys | Sort-Object

if ($readmeInsertionIndex -ne -1) {
    $newLines = @()
    foreach ($name in $sortedCrateNames) {
        $newLines += $cratesList[$name]
    }
    $pre = $readmeLines[0..($readmeInsertionIndex-1)]
    $post = $readmeLines[($readmeEndIndex+1)..$readmeLines.Length]
    $newReadmeContent = ($pre + $newLines + $post) -join [System.Environment]::NewLine
    Set-Content -Path $readmePath -Value $newReadmeContent
    Write-Host "Updated root README.md"
}


# Update root CHANGELOG.md
$changelogPath = Join-Path $repoRoot "CHANGELOG.md"
$changelogLines = Get-Content $changelogPath
$changelogCrates = @()
$changelogInsertionIndex = -1
$changelogEndIndex = -1

for ($i = 0; $i -lt $changelogLines.Length; $i++) {
    if ($changelogLines[$i] -match '^- \[`(.*)`\]\(.*/CHANGELOG.md\)') {
        if($changelogInsertionIndex -eq -1) {
            $changelogInsertionIndex = $i
        }
        $changelogCrates += $Matches[1]
        $changelogEndIndex = $i
    }
    elseif ($changelogInsertionIndex -ne -1) {
        # break if we are past the list and hit a non-empty line that's not a list item
        if ($changelogLines[$i].Trim() -ne "") {
            break
        }
    }
}

$changelogCrates += $crateName
$changelogCrates = $changelogCrates | Sort-Object

if ($changelogInsertionIndex -ne -1) {
    $newLines = @()
    foreach ($name in $changelogCrates) {
        $newLines += ('- [`{0}`](./crates/{0}/CHANGELOG.md)' -f $name)
    }
    $pre = $changelogLines[0..($changelogInsertionIndex-1)]
    $post = $changelogLines[($changelogEndIndex+1)..$changelogLines.Length]
    $newChangelogContent = ($pre + $newLines + $post) -join [System.Environment]::NewLine
    Set-Content -Path $changelogPath -Value $newChangelogContent
    Write-Host "Updated root CHANGELOG.md"
}

# Update root Cargo.toml
$cargoTomlPath = Join-Path $repoRoot "Cargo.toml"
$cargoTomlLines = Get-Content $cargoTomlPath
$localDeps = @{}
$inDepsSection = $false
$depsInsertionIndex = -1
$depsEndIndex = -1

for ($i = 0; $i -lt $cargoTomlLines.Length; $i++) {
    if ($cargoTomlLines[$i] -eq "# local dependencies") {
        $inDepsSection = $true
        $depsInsertionIndex = $i + 1
        continue
    }
    if ($inDepsSection) {
        if ($cargoTomlLines[$i] -match '^(\w+)\s*=\s*\{.*path\s*=\s*".*".*\}') {
            $localDeps[$Matches[1]] = $cargoTomlLines[$i]
            $depsEndIndex = $i
        } elseif ($cargoTomlLines[$i].Trim() -eq "" -or $cargoTomlLines[$i].StartsWith("#")) {
            break
        }
    }
}

$localDeps[$crateName] = "$crateName = { path = ""crates/$crateName"", default-features = false, version = ""0.1.0"" }"
$sortedDepNames = $localDeps.Keys | Sort-Object

if ($depsInsertionIndex -ne -1) {
    $newLines = @()
    foreach ($name in $sortedDepNames) {
        $newLines += $localDeps[$name]
    }
    $pre = $cargoTomlLines[0..($depsInsertionIndex-1)]
    $post = $cargoTomlLines[($depsEndIndex+1)..$cargoTomlLines.Length]
    $newCargoContent = ($pre + $newLines + $post) -join [System.Environment]::NewLine
    Set-Content -Path $cargoTomlPath -Value $newCargoContent
    Write-Host "Updated root Cargo.toml"
}


Write-Host "Crate '$crateName' created successfully!"
Write-Host "Next steps:"
Write-Host "1. Review the generated files in 'crates/$crateName'."
Write-Host "2. Create new logo.png and favicon.ico files for the crate and place them in the crate's directory."
