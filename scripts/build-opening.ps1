param(
    [Parameter(Mandatory)][string]$Record,
    [Parameter(Mandatory)][string]$Output,
    [int]$MaxOpeningPlies = 4, [int]$TopK = 4, [int]$ScoreMargin = 100,
    [int]$Depth = 4, [long]$Nodes = 10000, [int]$MaxPositions = 1000,
    [int]$MaxFrontier = 4096, [int]$Workers = 1,
    [string]$Model, [string]$Profile, [switch]$Resume, [string]$Engine
)
$ErrorActionPreference = 'Stop'
if ($Workers -ne 1) { throw 'Opening generation currently supports one worker. Proof sharding is a separate command.' }
if (!$Engine) { $Engine = Join-Path (Split-Path -Parent $PSScriptRoot) 'target/release/rustmoku-book.exe' }
$frozenEngine = (Get-FileHash -LiteralPath $Engine -Algorithm SHA256).Hash.ToLowerInvariant()
$engineIdentity = & $Engine build-identity
if ($LASTEXITCODE -ne 0) { throw 'Cannot identify engine build.' }
$frozenModel = if ($Model) { (Get-FileHash -LiteralPath $Model -Algorithm SHA256).Hash } else { $null }
$command = if ($Resume) { 'resume' } else { 'build' }
$arguments = @($command, '--record', $Record, '--output', $Output,
    '--engine-build', $engineIdentity, '--max-plies', "$MaxOpeningPlies", '--top-k', "$TopK",
    '--score-margin', "$ScoreMargin", '--depth', "$Depth", '--nodes', "$Nodes",
    '--max-positions', "$MaxPositions", '--max-frontier', "$MaxFrontier")
if ($Model) { $arguments += @('--model', $Model) }
if ($Profile) { $arguments += @('--profile', $Profile) }
& $Engine @arguments
if ($LASTEXITCODE -ne 0) { throw "Opening build failed ($LASTEXITCODE)." }
if ((Get-FileHash -LiteralPath $Engine -Algorithm SHA256).Hash.ToLowerInvariant() -ne $frozenEngine) { throw 'Executable changed during generation; reject this artifact.' }
if ($Model -and (Get-FileHash -LiteralPath $Model -Algorithm SHA256).Hash -ne $frozenModel) { throw 'Model changed during generation; reject this artifact.' }
