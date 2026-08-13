# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Changelog generation for the release tooling.

.DESCRIPTION
    Deterministic CHANGELOG.md generation reused by scripts/release-changelog.ps1
    (the small helper the AI release skill forwards changelog work to). Extracted
    from the retired interactive release driver so the only release logic that
    remains in scripts/ is the mechanical, format-heavy work the prompt should not
    re-derive by hand: grouping conventional commits into sections, rendering PR
    links, folding `## Unreleased`, and emitting cascade "Now requires X of Y"
    bullets. All planning/cascade/version logic now lives in the release prompt.

    Depends on scripts/lib/releasing.ps1 for Invoke-Git, Get-FileLineEnding and the
    conventional-commit / PR-reference regexes.
#>

. "$PSScriptRoot/releasing.ps1"

# Maps commit types (e.g., 'chore') to a common group key (e.g., 'task').
$script:TypeGroupMapping = @{
    'chore' = 'task';
    'doc'   = 'docs';
    'misc'  = 'miscellaneous';
}

# Maps the final group key to a user-friendly header in the changelog.
$script:HeaderNameMapping = @{
    'breaking'      = '⚠️ Breaking';
    'build'         = '🏗️ Build System';
    'ci'            = '🔄 Continuous Integration';
    'docs'          = '📚 Documentation';
    'feat'          = '✨ Features';
    'fix'           = '🐛 Bug Fixes';
    'miscellaneous' = '🧩 Miscellaneous';
    'perf'          = '⚡ Performance';
    'refactor'      = '♻️ Code Refactoring';
    'style'         = '🎨 Styling';
    'task'          = '✔️ Tasks';
}

# Defines the preferred order for commit type sections in the changelog.
$script:TypeOrder = @('breaking', 'feat', 'fix', 'perf', 'docs', 'task', 'refactor', 'build', 'ci', 'style')

# Defines commit types that should be excluded from the changelog.
$script:IgnoredTypes = @('test')

function Sort-KeysByPreferredOrder {
    param(
        [string[]]$allKeys,
        [string[]]$preferredOrder
    )
    $sortedKeys = [System.Collections.ArrayList]::new()
    $remainingKeys = [System.Collections.ArrayList]::new()
    $remainingKeys.AddRange($allKeys)

    foreach ($key in $preferredOrder) {
        if ($remainingKeys.Contains($key)) {
            $null = $sortedKeys.Add($key)
            $null = $remainingKeys.Remove($key)
        }
    }

    $remainingKeys.Sort()
    $sortedKeys.AddRange($remainingKeys)
    return $sortedKeys
}

function Format-ConventionalCommits {
    param(
        [string[]]$rawCommitMessages,
        [string]$prBaseUrl
    )

    if (-not $rawCommitMessages) {
        return @()
    }

    $groupedCommits = [ordered]@{}

    foreach ($message in $rawCommitMessages) {
        $type = "miscellaneous"
        $description = $message
        $isConventional = $false

        $conventionalMatch = $script:ConventionalCommitRegex.Match($message)
        $isBreaking = $false
        if ($conventionalMatch.Success) {
            $type = $conventionalMatch.Groups[1].Value
            $isBreaking = $conventionalMatch.Groups[2].Value -eq '!'
            $description = $conventionalMatch.Groups[3].Value
            $isConventional = $true
        }

        if ($isConventional -and $script:IgnoredTypes -contains $type) {
            continue
        }

        if (-not [string]::IsNullOrEmpty($prBaseUrl)) {
            $prMatch = $script:PrReferenceRegex.Match($description)
            if ($prMatch.Success) {
                $fullMatch = $prMatch.Groups[0].Value
                $prNumber  = $prMatch.Groups[2].Value
                $prLink    = " ([#$prNumber]($prBaseUrl/$prNumber))"
                $description = $description.Substring(0, $description.Length - $fullMatch.Length) + $prLink
            }
        }

        # Breaking changes are grouped separately, regardless of the commit type
        $groupKey = if ($isBreaking) {
            'breaking'
        } elseif ($script:TypeGroupMapping.ContainsKey($type)) {
            $script:TypeGroupMapping[$type]
        } else {
            $type
        }

        if (-not $groupedCommits.Contains($groupKey)) {
            $groupedCommits[$groupKey] = [System.Collections.ArrayList]::new()
        }

        [void]$groupedCommits[$groupKey].Add("  - $description")
    }

    $sortedKeys = Sort-KeysByPreferredOrder -allKeys $groupedCommits.Keys -preferredOrder $script:TypeOrder
    $formattedLines = @()
    foreach ($type in $sortedKeys) {
        if ($groupedCommits[$type].Count -gt 0) {
            $headerName = if ($script:HeaderNameMapping.ContainsKey($type)) { $script:HeaderNameMapping[$type] } else { $type.Substring(0, 1).ToUpper() + $type.Substring(1) }
            $formattedLines += @("- $headerName", "") + @($groupedCommits[$type]) + @("")
        }
    }

    if ($formattedLines.Count -gt 0 -and [string]::IsNullOrWhiteSpace($formattedLines[-1])) {
        if ($formattedLines.Count -gt 1) {
            $formattedLines = $formattedLines[0..($formattedLines.Count - 2)]
        } else {
            $formattedLines = @()
        }
    }

    return $formattedLines
}

function Extract-UnreleasedSection {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Content
    )

    if ([string]::IsNullOrEmpty($Content)) {
        return $null
    }

    # (?ims) — Multiline (^ matches line starts) + Singleline (. matches
    # newlines, so the non-greedy body can span lines) + IgnoreCase.
    $pattern = '(?ims)^##[ \t]+(?:\[Unreleased\]|Unreleased)[ \t]*\r?\n(?<body>.*?)(?=^##[ \t]|\z)'
    $match = [regex]::Match($Content, $pattern)
    if (-not $match.Success) {
        return $null
    }

    $body = $match.Groups['body'].Value
    $lines = @($body -split "`r?`n")

    # Strip trailing blank lines.
    while ($lines.Count -gt 0 -and [string]::IsNullOrWhiteSpace($lines[-1])) {
        $lines = if ($lines.Count -eq 1) { @() } else { @($lines[0..($lines.Count - 2)]) }
    }
    # Strip leading blank lines.
    while ($lines.Count -gt 0 -and [string]::IsNullOrWhiteSpace($lines[0])) {
        $lines = if ($lines.Count -eq 1) { @() } else { @($lines[1..($lines.Count - 1)]) }
    }

    return [pscustomobject]@{
        BodyLines             = [string[]]$lines
        ContentWithoutSection = $Content.Remove($match.Index, $match.Length)
    }
}

function Write-Changelog {
    param(
        [string]$packageName,
        [string]$newVersion,
        [string]$packageFolder,
        [string]$changelogFile,
        [string]$prBaseUrl,
        # Optional: when this package is being released as a cascade-from-dependency,
        # describe one or more cascades so a maintenance/breaking entry can be
        # written even if the package has no commits since its last release. Each
        # element shape: @{ Target = '<name>'; Version = '<x.y.z>'; Breaking = $false }.
        # The section header is `⚠️ Breaking` if ANY reason is Breaking, otherwise
        # `🔧 Maintenance`; one bullet is emitted per reason in deterministic
        # (Target-sorted) order. Element shape is duck-typed (.Target / .Version /
        # .Breaking) so both hashtables and [pscustomobject] are accepted.
        [object[]]$cascadeReasons = $null
    )

    $hasCascade = ($null -ne $cascadeReasons) -and ($cascadeReasons.Count -gt 0)

    # Read the existing changelog up front and extract any `## Unreleased`
    # section. The body of that section will be folded into the new version
    # section we're about to create — leaving it behind would orphan
    # manually-curated release notes below the freshly-inserted version
    # heading. Unreleased presence alone is enough reason to write a new
    # section, so we check it in the no-content guard below.
    $existingContent          = $null
    $existingHadContent       = $false
    $unreleasedLines          = @()
    if (Test-Path $changelogFile) {
        $existingContent = Get-Content $changelogFile -Raw
        if ($existingContent) {
            $existingHadContent = $true
            $extracted = Extract-UnreleasedSection -Content $existingContent
            if ($null -ne $extracted) {
                $unreleasedLines  = $extracted.BodyLines
                $existingContent  = $extracted.ContentWithoutSection
            }
        }
    }

    $hasUnreleased = $unreleasedLines.Count -gt 0

    $tags = Invoke-Git -Arguments @('tag', '--list', "$packageName-v*")
    $latestTag = $null
    if ($null -eq $tags -or $tags.Count -eq 0) {
        Write-Warning "No tags found for package '$packageName'. Generating changelog from all history."
    } else {
        $filteredTags = @($tags | Where-Object { $_ -match "^${packageName}-v\d+\.\d+\.\d+$" })
        if ($filteredTags.Count -gt 0) {
            $sortedTags = @($filteredTags | Sort-Object { [version]($_ -replace "${packageName}-v", '') })
            $latestTag = $sortedTags[-1]
        } else {
            Write-Warning "No valid semantic version tags found for package '$packageName'. Generating changelog from all history."
        }
    }

    $currentDate = (Get-Date).ToString('yyyy-MM-dd')

    # Get commits since the latest tag (unreleased commits)
    $range = if ($latestTag) { "$latestTag..HEAD" } else { "HEAD" }
    $rawCommits = Invoke-Git -Arguments @('log', $range, '--pretty=format:%s', '--', $packageFolder)
    if ($null -eq $rawCommits -or $rawCommits.Count -eq 0) {
        $rawCommits = @()
    } else {
        $rawCommits = @($rawCommits)
    }

    $formattedCommits = @()
    if ($rawCommits.Count -gt 0) {
        $formattedCommits = Format-ConventionalCommits -rawCommitMessages $rawCommits -prBaseUrl $prBaseUrl
    }

    if ($formattedCommits.Count -eq 0 -and -not $hasCascade -and -not $hasUnreleased) {
        if ($rawCommits.Count -eq 0) {
            Write-Warning "No unreleased commits found to add to the changelog."
        } else {
            $filteredCount = $rawCommits.Count
            $noun = if ($filteredCount -eq 1) { 'commit was' } else { 'commits were' }
            Write-Warning "No relevant commits found to add to the changelog (all $filteredCount $noun filtered out)."
        }
        return
    }

    # Prepend cascade entries when this package is being released because one
    # (or more) of its dependencies was released. Emits structured
    # "Now requires <version> of <target>" bullets — deliberately formal
    # rather than colloquial — under the appropriate section:
    #   - 🔧 Maintenance        (when no contributing cascade is breaking)
    #   - ⚠️ Breaking           (when at least one contributing cascade is breaking)
    # Bullets are sorted by Target name for deterministic output across runs.
    # If the same section header was already produced by
    # Format-ConventionalCommits for this release, the cascade bullets are
    # merged into that existing section instead of creating a duplicate header.
    if ($hasCascade) {
        $anyBreaking = $false
        foreach ($r in $cascadeReasons) {
            if ([bool]$r.Breaking) { $anyBreaking = $true; break }
        }
        $sectionHeader = if ($anyBreaking) { '- ⚠️ Breaking' } else { '- 🔧 Maintenance' }

        $sortedReasons = @($cascadeReasons | Sort-Object -Property @{ Expression = { $_.Target } })
        $cascadeBullets = @($sortedReasons | ForEach-Object {
            "  - Now requires ``$($_.Version)`` of ``$($_.Target)``"
        })

        $existingHeaderIdx = -1
        for ($i = 0; $i -lt $formattedCommits.Count; $i++) {
            if ($formattedCommits[$i] -eq $sectionHeader) {
                $existingHeaderIdx = $i
                break
            }
        }

        if ($existingHeaderIdx -ge 0) {
            # Find the end of this section (next top-level "- " header or end of list).
            $insertIdx = $formattedCommits.Count
            for ($i = $existingHeaderIdx + 1; $i -lt $formattedCommits.Count; $i++) {
                if ($formattedCommits[$i] -match '^- \S') { $insertIdx = $i; break }
            }
            # Trim trailing blank lines belonging to the section.
            while ($insertIdx -gt $existingHeaderIdx + 1 -and [string]::IsNullOrWhiteSpace($formattedCommits[$insertIdx - 1])) {
                $insertIdx--
            }
            $before = if ($insertIdx -gt 0) { @($formattedCommits[0..($insertIdx - 1)]) } else { @() }
            $after  = if ($insertIdx -lt $formattedCommits.Count) { @($formattedCommits[$insertIdx..($formattedCommits.Count - 1)]) } else { @() }
            $formattedCommits = $before + $cascadeBullets + $after
        } else {
            $cascadeLines = @($sectionHeader, "") + $cascadeBullets
            if ($formattedCommits.Count -gt 0) {
                $formattedCommits = $cascadeLines + @("") + $formattedCommits
            } else {
                $formattedCommits = $cascadeLines
            }
        }
    }

    # Build the new version section. User-curated `## Unreleased` body lines
    # (if any) lead the section so the manually-authored narrative appears
    # first; cascade bullets + commit-derived bullets follow as supplementary
    # detail. A blank line separates the two groups when both are present.
    $newVersionSection = @("## [$newVersion] - $currentDate", "")
    if ($hasUnreleased) {
        $newVersionSection += $unreleasedLines
        if ($formattedCommits.Count -gt 0) {
            $newVersionSection += ""
        }
    }
    $newVersionSection += $formattedCommits
    $newVersionSection += ""

    # Insert the new version section into the existing changelog, using the
    # Unreleased-stripped content as the base (so the orphaned `## Unreleased`
    # heading is no longer present in the output).
    if ($existingHadContent) {
        # Find the position after "# Changelog" header and any blank lines
        # Insert the new version section there
        $headerPattern = '^# Changelog\s*\r?\n(\r?\n)*'
        if ($existingContent -match $headerPattern) {
            # Match the existing file's line-ending convention so we don't introduce
            # mixed endings (e.g. CRLF body + LF for the new section).
            $eol = Get-FileLineEnding -Path $changelogFile
            $headerMatch = [regex]::Match($existingContent, $headerPattern)
            $insertPosition = $headerMatch.Index + $headerMatch.Length
            $newContent = $existingContent.Substring(0, $insertPosition) +
                          ($newVersionSection -join $eol) + $eol +
                          $existingContent.Substring($insertPosition)
            Set-Content -LiteralPath $changelogFile -Value $newContent -NoNewline -Encoding utf8
            Write-Host "✅ Changelog updated at '$changelogFile'."
            return
        }
    }

    # If no existing changelog or couldn't parse it, create a new one.
    # No existing file to sample from, so default to LF (modern convention; matches
    # what .gitattributes normalizes to in repos that enforce it).
    $changelogContent = @("# Changelog", "")
    $changelogContent += $newVersionSection
    Set-Content -LiteralPath $changelogFile -Value (($changelogContent -join "`n") + "`n") -NoNewline -Encoding utf8
    Write-Host "✅ Changelog created at '$changelogFile'."
}
