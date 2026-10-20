# Command reference

Execute these examples in PowerShell, in stage order. Replace the placeholders
before execution. Each block stops on failed native commands except the explicitly
advisory registry discovery. Retain the original diagnostic and inspect partial
writes before retrying a failed stage.

| Placeholder | Meaning |
| --- | --- |
| `TOOL` | Installed `cargo-release-plan` executable or the selected tested source-built executable. |
| `MODE` | `configured` for toolkit publication, or `standalone` for an explicitly requested one-off version-increment PR. |
| `MANIFEST` | Absolute path to the selected workspace's `Cargo.toml`. |
| `CONFIG` | Configured mode only: publication configuration path, relative to the selected workspace. Leave empty in standalone mode. |
| `HISTORY_REF` | Standalone mode only: user-selected local ref or commit delimiting actual release history. |
| `WORK_DIR` | New absolute ignored or external directory for assessment evidence. |
| `VERIFY_DIR` | Separate new absolute ignored or external directory for final verification; do not create it manually. |
| `MERGE_TARGET_REF` | Unmerged parent PR's actual target ref, or an empty string for release-branch assessment. |
| `HISTORY_COMMIT` | Full history commit from configured context or the standalone input record. |
| `TARGET_COMMIT` | Full target commit from the selected initialization path, or an empty string when absent. |
| `PACKAGE` | Package name from a report entry. |
| `DIFF_PATH` | That entry's `diff_path`: the generated patch filename relative to the directory containing `report.json`; absent when there is no file patch. |

For repository-local evidence directories, run this for each path before writing:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
git check-ignore --quiet -- "{{WORK_DIR}}"
git check-ignore --quiet -- "{{VERIFY_DIR}}"
```

A nonzero result does not establish an ignored path. Choose external directories
or an existing ignored location instead. Do not run these checks for external paths.

## Stage 1: Check prerequisites and select history

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
if ("{{MODE}}" -notin @("configured", "standalone")) { throw "Select an explicit planning mode." }
$Identity = & "{{TOOL}}" version | ConvertFrom-Json
$Expected = @{ plan = 8; report = 8; prepared = 8; decisions = 2; compatibility = 3 }
if ("{{MODE}}" -eq "configured") { $Expected.release_context = 2 }
foreach ($Name in $Expected.Keys) {
    if ($Identity.schemas.$Name -ne $Expected[$Name]) {
        throw "Unsupported $Name schema; follow the skill upgrade instructions."
    }
}
Get-Command cargo-semver-checks -ErrorAction Stop | Out-Null
$Branch = (git symbolic-ref --quiet --short HEAD).Trim()
New-Item -ItemType Directory -Path "{{WORK_DIR}}" | Out-Null
```

Run exactly one initialization path below.

### Configured initialization

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$Branch = (git symbolic-ref --quiet --short HEAD).Trim()
$TargetArguments = @()
if (-not [string]::IsNullOrWhiteSpace("{{MERGE_TARGET_REF}}")) {
    $TargetArguments = @("--merge-target", "{{MERGE_TARGET_REF}}")
}
& "{{TOOL}}" release-context --manifest-path "{{MANIFEST}}" --config "{{CONFIG}}" `
    @TargetArguments --verbose > "{{WORK_DIR}}\context.json"
$Context = Get-Content -LiteralPath "{{WORK_DIR}}\context.json" -Raw | ConvertFrom-Json
if ($Branch -eq $Context.release_branch) { throw "Run version planning on a feature branch." }
```

Use context's full history/target commit IDs as `HISTORY_COMMIT` and `TARGET_COMMIT`.
Record the original target ref for Stage 6.

Only configured mode performs advisory registry discovery:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false
& "{{TOOL}}" check-published --manifest-path "{{MANIFEST}}" --verbose
if ($LASTEXITCODE -ne 0) {
    Write-Warning "Registry discovery was incomplete. Retain its diagnostics and assess the reported packages."
}
```

### Standalone initialization

The user supplies the local history ref and, for a stack, the parent target ref.
Fetch those refs beforehand only if the user requests a remote refresh. Do not
invent a GitHub repository, workflow, publication configuration or registry setup.

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$Workspace = Split-Path -Parent "{{MANIFEST}}"
$HistoryRef = "{{HISTORY_REF}}"
if ([string]::IsNullOrWhiteSpace($HistoryRef)) { throw "Supply the user's release-history ref." }
$TargetRef = "{{MERGE_TARGET_REF}}"
$HistoryCommit = (git -C $Workspace rev-parse --verify --end-of-options "$HistoryRef^{commit}").Trim()
$TargetCommit = $null
if (-not [string]::IsNullOrWhiteSpace($TargetRef)) {
    $TargetCommit = (git -C $Workspace rev-parse --verify --end-of-options "$TargetRef^{commit}").Trim()
}
[ordered]@{
    release_history_ref = $HistoryRef
    release_history = $HistoryCommit
    merge_target_ref = $TargetRef
    merge_target = $TargetCommit
} | ConvertTo-Json | Set-Content -LiteralPath "{{WORK_DIR}}\standalone-inputs.json"
```

Use the resolved values as `HISTORY_COMMIT` and `TARGET_COMMIT`. The input record
is an agent-authored handoff of the user's selections, not a publication config or
tool context artifact. Preparation validates the selected history/target relation.
Do not run `release-context` or `check-published` in this mode. The version query
and schema checks above still apply.
Confirm that the named checkout branch is the intended PR work branch before
preparation; do not apply directly to its destination branch.

## Stage 2: Prepare complete evidence

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$TargetArguments = @()
if (-not [string]::IsNullOrWhiteSpace("{{TARGET_COMMIT}}")) {
    $TargetArguments = @("--merge-target", "{{TARGET_COMMIT}}")
}
& "{{TOOL}}" prepare --manifest-path "{{MANIFEST}}" --release-history "{{HISTORY_COMMIT}}" `
    @TargetArguments --output "{{WORK_DIR}}" --verbose
& "{{TOOL}}" check-compatibility --manifest-path "{{MANIFEST}}" `
    --prepared "{{WORK_DIR}}\prepared.json" --output "{{WORK_DIR}}\compatibility" --verbose
```

Read `report.json`, its `diffs` patches, `prepared.json`, and the compatibility
evidence in `WORK_DIR`. A consistent lockfile may be installed during preparation;
the report describes that same source. Compatibility checks require completed
evidence before semantic assessment.
Completed evidence can contain explicit exclusions: `not_compared_reason` distinguishes
`unsupported-proc-macro` from `no-baseline`. Neither implies compatibility. Keep these
packages in the dependency-first assessment; do not substitute the checker target list.

## Stage 3: Assess in dependency order

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
& "{{TOOL}}" analysis-order --report "{{WORK_DIR}}\report.json" --verbose `
    > "{{WORK_DIR}}\analysis-order.json"
```

Use the returned ordered batches for the assessment. Resolve a package's patch
with `Join-Path "{{WORK_DIR}}" "{{DIFF_PATH}}"` when it has `diff_path`.

## Stage 4: Choose semantic impacts

Write the assessed decisions to `WORK_DIR\decisions.json`, for example:

```json
{
  "schema_version": 2,
  "changes": [
    { "name": "example-api", "impact": "nonbreaking" },
    { "name": "example-tool", "impact": "patch" }
  ]
}
```

Names and impacts are scenario values, not defaults. Use the decision guide and
report evidence; keep the explanation alongside this agent-authored document.
For re-exported APIs, record the affected public package and consumer-visible change
even when its own source is unchanged. A passing facade check is not coverage of
the dependency's definitions or generated code.

## Stage 5: Review resolved effects

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
& "{{TOOL}}" propose --report "{{WORK_DIR}}\report.json" `
    --decisions "{{WORK_DIR}}\decisions.json" --out "{{WORK_DIR}}\plan.json" --verbose
& "{{TOOL}}" preview --manifest-path "{{MANIFEST}}" --prepared "{{WORK_DIR}}\prepared.json" `
    --plan "{{WORK_DIR}}\plan.json" --output "{{WORK_DIR}}\preview" --verbose
& "{{TOOL}}" check-compatibility --manifest-path "{{MANIFEST}}" `
    --plan "{{WORK_DIR}}\preview\plan.json" --output "{{WORK_DIR}}\preview-compatibility" --verbose
```

Read the complete `preview\report.json`, patches, resolved `preview\plan.json`
and completed preview compatibility evidence. Repeating this stage requires new
preview and compatibility output directories; do not overwrite retained evidence.

## Stage 6: Refresh and apply

Preserve the pre-application file state, then execute the matching mode's complete
refresh-and-apply block. Do not resume at `apply` alone after a failed refresh or gate.

### Configured refresh

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$TargetArguments = @()
if (-not [string]::IsNullOrWhiteSpace("{{MERGE_TARGET_REF}}")) {
    $TargetArguments = @("--merge-target", "{{MERGE_TARGET_REF}}")
}
& "{{TOOL}}" release-context --manifest-path "{{MANIFEST}}" --config "{{CONFIG}}" `
    @TargetArguments --verbose > "{{WORK_DIR}}\refreshed-context.json"
$Original = Get-Content -LiteralPath "{{WORK_DIR}}\context.json" -Raw | ConvertFrom-Json
$Current = Get-Content -LiteralPath "{{WORK_DIR}}\refreshed-context.json" -Raw | ConvertFrom-Json
if ($Original.release_history -cne $Current.release_history -or
    $Original.merge_target -cne $Current.merge_target) {
    throw "Release history or merge target moved; restart assessment using the recovery instructions."
}
& "{{TOOL}}" check-published --manifest-path "{{MANIFEST}}" `
    --plan "{{WORK_DIR}}\preview\plan.json" --verbose
& "{{TOOL}}" apply --manifest-path "{{MANIFEST}}" --plan "{{WORK_DIR}}\preview\plan.json" --verbose
```

Only a successful plan-scoped registry check permits configured application;
missing or unknown identities are first-publication or access blockers.

### Standalone refresh

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$Workspace = Split-Path -Parent "{{MANIFEST}}"
$Original = Get-Content -LiteralPath "{{WORK_DIR}}\standalone-inputs.json" -Raw | ConvertFrom-Json
$HistoryRef = $Original.release_history_ref
$CurrentHistory = (git -C $Workspace rev-parse --verify --end-of-options "$HistoryRef^{commit}").Trim()
$CurrentTarget = $null
if (-not [string]::IsNullOrWhiteSpace($Original.merge_target_ref)) {
    $TargetRef = $Original.merge_target_ref
    $CurrentTarget = (git -C $Workspace rev-parse --verify --end-of-options "$TargetRef^{commit}").Trim()
}
if ($Original.release_history -cne $CurrentHistory -or
    $Original.merge_target -cne $CurrentTarget) {
    throw "User-selected history or target moved; restart assessment."
}
& "{{TOOL}}" apply --manifest-path "{{MANIFEST}}" --plan "{{WORK_DIR}}\preview\plan.json" --verbose
```

No publication-readiness gate is added to standalone version edits.

## Stage 7: Verify and hand off

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$ConfigArguments = @()
switch ("{{MODE}}") {
    "configured" { $ConfigArguments = @("--config", "{{CONFIG}}") }
    "standalone" {}
    default { throw "Select an explicit planning mode." }
}
$TargetArguments = @()
if (-not [string]::IsNullOrWhiteSpace("{{TARGET_COMMIT}}")) {
    $TargetArguments = @("--merge-target", "{{TARGET_COMMIT}}")
}
cargo metadata --manifest-path "{{MANIFEST}}" --locked --format-version 1 > $null
& "{{TOOL}}" check-compatibility --manifest-path "{{MANIFEST}}" `
    --release-history "{{HISTORY_COMMIT}}" @TargetArguments --output "{{VERIFY_DIR}}" --verbose
& "{{TOOL}}" check --manifest-path "{{MANIFEST}}" --release-history "{{HISTORY_COMMIT}}" `
    @TargetArguments @ConfigArguments --format github --verbose
```

Read `VERIFY_DIR\report.json` and completed compatibility evidence, verify all
decisions and resolved targets, and confirm that intended release inputs are
tracked. The metadata exit status verifies the lockfile; its JSON is not a second
assessment report. Keep the reviewed release plan and handoff current. Standalone
verification does not validate binary publication metadata or require registry
bootstrap. Present its version-increment PR for normal review and authorized merge;
do not continue into publication commands.
