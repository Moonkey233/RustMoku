param(
    [ValidateSet('smoke','pilot','serious','large')][string]$Preset = 'smoke',
    [string]$Config, [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$Dataset, [string]$Device = 'cpu', [int]$BatchSize,
    [string]$Resume, [long]$Seed = 1, [string]$TeacherModel, [string]$TeacherProfile,
    [int]$Games, [int]$Steps, [int]$Epochs,
    [string]$Engine, [string]$Python = 'python'
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (!$Config) { $Config = Join-Path $repoRoot "training/presets/$Preset.json" }
if (!$Engine) { $Engine = Join-Path $repoRoot 'target/release/rustmoku-data.exe' }
$arguments = @('-X','utf8',(Join-Path $repoRoot 'training/production.py'),
    '--config',$Config,'--output',$OutputDirectory,'--engine',$Engine,'--device',$Device,'--seed',"$Seed")
foreach ($pair in @(@('dataset',$Dataset),@('resume',$Resume),@('teacher-model',$TeacherModel),@('teacher-profile',$TeacherProfile))) {
    if ($pair[1]) { $arguments += @("--$($pair[0])",$pair[1]) }
}
foreach ($pair in @(@('batch-size',$BatchSize),@('games',$Games),@('steps',$Steps),@('epochs',$Epochs))) {
    if ($pair[1]) { $arguments += @("--$($pair[0])","$($pair[1])") }
}
& $Python @arguments
if ($LASTEXITCODE -ne 0) { throw "Production pipeline failed ($LASTEXITCODE)." }
