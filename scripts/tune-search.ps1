param(
    [Parameter(Mandatory)][string]$Config,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [switch]$Run, [string]$Python = 'python'
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$arguments = @('-X', 'utf8', (Join-Path $repoRoot 'training/tune_search.py'), '--config', $Config, '--output', $OutputDirectory)
if ($Run) { $arguments += '--run' }
& $Python @arguments
if ($LASTEXITCODE -ne 0) { throw "Search tuning failed ($LASTEXITCODE)." }
