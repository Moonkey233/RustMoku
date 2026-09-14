param(
    [Parameter(Mandatory)][string]$Record,
    [Parameter(Mandatory)][ValidateSet('black','white')][string]$Attacker,
    [Parameter(Mandatory)][string]$Checkpoint,
    [Parameter(Mandatory)][string]$Output,
    [long]$Nodes = 1000, [double]$Seconds = 60,
    [int]$RamMiB = 32, [long]$DiskMiB = 1024, [int]$MaxDepth = 225,
    [switch]$Resume, [string]$Engine, [string]$Python = 'python'
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (!$Engine) { $Engine = Join-Path $repoRoot 'target/release/rustmoku-solver.exe' }
$arguments = @('-X','utf8','-m','offline.solve','--engine',$Engine,'--record',$Record,
    '--attacker',$Attacker,'--checkpoint',$Checkpoint,'--output',$Output,
    '--nodes',"$Nodes",'--seconds',"$Seconds",'--ram-mib',"$RamMiB",'--disk-mib',"$DiskMiB",'--max-depth',"$MaxDepth")
if ($Resume) { $arguments += '--resume' }
Push-Location $repoRoot
try {
    & $Python @arguments
    if ($LASTEXITCODE -ne 0) { throw "Disk proof command failed ($LASTEXITCODE)." }
} finally { Pop-Location }
