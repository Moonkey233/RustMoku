param(
    [Parameter(Mandatory)][ValidateSet('frontier','solve-batch','merge')][string]$Command,
    [Parameter(Mandatory)][string]$Manifest,
    [string]$Checkpoint, [string]$Config, [string]$Output, [string[]]$Inputs,
    [int]$Shards = 1, [int]$Shard = 0, [int]$Workers = 1, [int]$Limit = 256,
    [long]$Nodes = 1000, [double]$Seconds = 60, [int]$RamMiB = 32, [long]$DiskMiB = 1024,
    [switch]$Resume, [string]$Engine, [string]$Python = 'python'
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (!$Engine) { $Engine = Join-Path $repoRoot 'target/release/rustmoku-solver.exe' }
$arguments = @('-X','utf8','-m','offline.shards',$Command,'--engine',$Engine,'--manifest',$Manifest,
    '--shards',"$Shards",'--shard',"$Shard",'--workers',"$Workers",'--limit',"$Limit",
    '--nodes',"$Nodes",'--seconds',"$Seconds",'--ram-mib',"$RamMiB",'--disk-mib',"$DiskMiB")
foreach ($pair in @(@('checkpoint',$Checkpoint),@('config',$Config),@('output',$Output))) {
    if ($pair[1]) { $arguments += @("--$($pair[0])",$pair[1]) }
}
if ($Inputs) { $arguments += '--inputs'; $arguments += $Inputs }
if ($Resume) { $arguments += '--resume' }
Push-Location $repoRoot
try {
    & $Python @arguments
    if ($LASTEXITCODE -ne 0) { throw "Proof shard command failed ($LASTEXITCODE)." }
} finally { Pop-Location }
