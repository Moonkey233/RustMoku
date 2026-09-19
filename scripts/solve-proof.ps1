param(
    [Parameter(Mandatory)][string]$Record,
    [Parameter(Mandatory)][ValidateSet('black','white')][string]$Attacker,
    [Parameter(Mandatory)][string]$Checkpoint,
    [Parameter(Mandatory)][string]$Output,
    [long]$Nodes = 1000, [double]$Seconds = 60,
    [Alias('RamMiB')][int]$SqliteCacheMiB = 32, [long]$DiskMiB = 1024, [int]$MaxAdditionalPlies = 225,
    [int]$VcfPlies = 8, [long]$VcfNodes = 1000, [int]$VctPlies = 6, [long]$VctNodes = 1000,
    [switch]$Resume, [string]$Engine, [string]$Python = 'python'
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (!$Engine) { $Engine = Join-Path $repoRoot 'target/release/rustmoku-solver.exe' }
$arguments = @('-X','utf8','-m','offline.solve','--engine',$Engine,'--record',$Record,
    '--attacker',$Attacker,'--checkpoint',$Checkpoint,'--output',$Output,
    '--nodes',"$Nodes",'--seconds',"$Seconds",'--sqlite-cache-mib',"$SqliteCacheMiB",'--disk-mib',"$DiskMiB",'--max-additional-plies',"$MaxAdditionalPlies")
if ($Resume) { $arguments += '--resume' }
$arguments += @('--vcf-plies', "$VcfPlies", '--vcf-nodes', "$VcfNodes", '--vct-plies', "$VctPlies", '--vct-nodes', "$VctNodes")
Push-Location $repoRoot
try {
    & $Python @arguments
    if ($LASTEXITCODE -ne 0) { throw "Disk proof command failed ($LASTEXITCODE)." }
} finally { Pop-Location }
