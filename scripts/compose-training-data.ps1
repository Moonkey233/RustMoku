param(
    [Parameter(Mandatory)][string]$Config,
    [Parameter(Mandatory)][string]$Output,
    [string]$Python = 'python'
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
& $Python -X utf8 (Join-Path $repoRoot 'training/compose.py') --config $Config --output $Output
if ($LASTEXITCODE -ne 0) { throw "Dataset composition failed ($LASTEXITCODE)." }
