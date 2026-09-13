param(
    [Parameter(Mandatory)][string]$Config,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$Python = 'python'
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
& $Python -X utf8 (Join-Path $repoRoot 'training/confirm.py') --config $Config --output $OutputDirectory
if ($LASTEXITCODE -ne 0) { throw "Confirmation failed ($LASTEXITCODE)." }
