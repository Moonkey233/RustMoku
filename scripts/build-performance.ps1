param([ValidateSet('portable','native')][string]$Mode = 'portable')
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$oldRustFlags = $env:RUSTFLAGS
$oldEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
Push-Location $repoRoot
try {
    # Encoded flags take precedence; reject ambiguity instead of silently losing
    # the requested ISA. Ordinary cargo build --release remains unchanged.
    if ($oldEncodedFlags) { throw 'Unset CARGO_ENCODED_RUSTFLAGS before using this build wrapper.' }
    if ($Mode -eq 'native') {
        $env:RUSTFLAGS = "$oldRustFlags -C target-cpu=native".Trim()
        cargo build --workspace --profile release-native
    } else {
        if ($oldRustFlags -match 'target-cpu|target-feature') { throw 'Portable build cannot use caller CPU feature overrides.' }
        cargo build --workspace --profile release-portable
    }
    if ($LASTEXITCODE -ne 0) { throw "Build failed ($LASTEXITCODE)." }
} finally {
    $env:RUSTFLAGS = $oldRustFlags
    Pop-Location
}
