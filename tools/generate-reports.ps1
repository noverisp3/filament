# Runs Minecraft's own data generator to produce `blocks.json`, `registries.json`
# and `packets.json` for the target version.
#
# This is the whole reason no Fabric mod is needed. Minecraft ships a data
# generator that dumps its registries to JSON, and `blocks.json` is exactly the
# block state table: every block, every state, every state's ID and properties.
# Those IDs are what a server puts on the wire in chunk data, so a server that
# gets them right renders correctly and a server that gets them wrong renders
# wrong terrain with no error anywhere.
#
# Valence gets the same information by running a Fabric mod inside a live game
# and reading `Registries.BLOCK.getRawId()`. That is more work and needs a
# running client. The data generator needs neither.
#
# Usage:
#   .\generate-reports.ps1
#   .\generate-reports.ps1 -Version 26.3

param(
    [string] $Version = '26.3',
    [string] $McRef  = (Join-Path $PSScriptRoot '..\..\mc-ref'),
    [string] $OutDir = (Join-Path $PSScriptRoot '..\extracted'),
    [int]    $TimeoutSeconds = 600
)

$ErrorActionPreference = 'Stop'

$jar    = Join-Path $McRef "jars\$Version-server.jar"
$libs   = Join-Path $McRef 'libraries'
$logDir = Join-Path $PSScriptRoot 'logs'

foreach ($p in @($jar, $libs)) {
    if (-not (Test-Path -LiteralPath $p)) { throw "Missing: $p" }
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
New-Item -ItemType Directory -Path $logDir -Force | Out-Null

# A fresh working directory each run. The generator writes into
# `generated/reports` relative to the current directory and will not overwrite
# a stale file, so reusing a dirty directory produces a mixture of two runs.
$work = Join-Path $env:TEMP "filament-reports-$Version"
if (Test-Path -LiteralPath $work) { Remove-Item -Recurse -Force -LiteralPath $work }
New-Item -ItemType Directory -Path $work -Force | Out-Null

$stdout = Join-Path $logDir "generate-$Version.out.log"
$stderr = Join-Path $logDir "generate-$Version.err.log"

Write-Output "[generate] version  = $Version"
Write-Output "[generate] jar      = $jar"
Write-Output "[generate] workdir  = $work"

# `bundlerMainClass` points the bundler wrapper at the data generator instead of
# the server, and `--reports` is the flag that makes it dump rather than run.
$argList = @(
    '-DbundlerMainClass=net.minecraft.data.Main',
    '-jar', $jar,
    '--reports'
)

$sw = [System.Diagnostics.Stopwatch]::StartNew()

# `Start-Process -PassThru` does not populate ExitCode on Windows PowerShell 5.1,
# for a process started this way: it comes back empty whether the run succeeded
# or crashed. That is indistinguishable from success if you compare it to zero,
# which is how a failed data generator turns into a confusing "no reports"
# message much later. Constructing the process object directly does record it.
$psi = [System.Diagnostics.ProcessStartInfo]::new()
$psi.FileName               = 'java'
$psi.Arguments              = ($argList -join ' ')
$psi.WorkingDirectory       = $work
$psi.UseShellExecute        = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError  = $true

$proc = [System.Diagnostics.Process]::Start($psi)

# Both pipes have to be drained while the process runs. A data generator writes
# enough to stderr to fill the OS buffer, and a full buffer deadlocks it.
$outTask = $proc.StandardOutput.ReadToEndAsync()
$errTask = $proc.StandardError.ReadToEndAsync()

if (-not $proc.WaitForExit($TimeoutSeconds * 1000)) {
    try { $proc.Kill($true) } catch { }
    throw "data generator did not finish within $TimeoutSeconds seconds"
}
$sw.Stop()

$exitCode = $proc.ExitCode
Set-Content -LiteralPath $stdout -Value $outTask.Result -Encoding utf8
Set-Content -LiteralPath $stderr -Value $errTask.Result -Encoding utf8

Write-Output "[generate] exit=$exitCode elapsed=$([int]$sw.Elapsed.TotalSeconds)s"

if ($exitCode -ne 0) {
    Write-Output '[generate] stderr:'
    if (Test-Path -LiteralPath $stderr) { Get-Content -LiteralPath $stderr -Tail 40 }
    throw "data generator failed with exit code $exitCode"
}

$reports = Join-Path $work 'generated\reports'
if (-not (Test-Path -LiteralPath $reports)) { throw "no reports directory at $reports" }

# Only the three reports this crate turns into Rust. The rest -- biome
# parameters, every component file, the command tree -- are several megabytes
# that nothing here reads yet.
$wanted = @('blocks.json', 'registries.json', 'packets.json')
foreach ($name in $wanted) {
    $src = Join-Path $reports $name
    if (-not (Test-Path -LiteralPath $src)) { throw "expected report missing: $name" }

    $dest = Join-Path $OutDir $name
    Copy-Item -LiteralPath $src -Destination $dest -Force

    $kb = [math]::Round((Get-Item -LiteralPath $dest).Length / 1KB)
    Write-Output "[generate] wrote $name ($kb KB)"
}

# The generated tree is several hundred megabytes and is thrown away on purpose.
# Keeping it would make the repo unreadable and the next run deletes it anyway.
Remove-Item -Recurse -Force -LiteralPath $work

Write-Output '[generate] done'