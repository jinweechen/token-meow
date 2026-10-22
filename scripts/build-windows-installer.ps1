#requires -Version 7.0
[CmdletBinding()]
param(
    [ValidateSet('Build', 'ReuseEngine')][string]$Mode = 'Build',
    [string]$EnginePath,
    [ValidatePattern('^$|^[A-Fa-f0-9]{64}$')][string]$EngineExpectedSha256,
    [Parameter(Mandatory)][string]$SevenZipPath,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$SevenZipExpectedSha256,
    [string]$SevenZipDllPath,
    [ValidatePattern('^$|^[A-Fa-f0-9]{64}$')][string]$SevenZipDllExpectedSha256,
    [string]$OutputDirectory,
    [string]$CargoTargetDirectory,
    [switch]$PreflightOnly
)

# Produces a branded installer only. Never executes an engine, App or uninstaller.
# OutputDirectory is a NEW run root; only its release/ directory is distributable.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$taskOutputRoot = Join-Path $taskRoot 'output'
$taskStreams = [Collections.Generic.List[IO.FileStream]]::new()
$taskInputs = [Collections.Generic.List[object]]::new()
$taskInputByPath = [Collections.Generic.Dictionary[string,object]]::new([StringComparer]::OrdinalIgnoreCase)
$taskCommands = [Collections.Generic.List[object]]::new()
$taskCliBinding = $null
$taskGuards = $null
$taskFinalizationGuards = $null
$taskCreated = $false
$taskClosed = $false

function Assert-NoReparse([string]$Path, [switch]$AllowMissing) {
    $taskCheck = [IO.Path]::GetFullPath($Path)
    while ($taskCheck) {
        if (Test-Path -LiteralPath $taskCheck) {
            $taskInfo = Get-Item -LiteralPath $taskCheck -Force
            if ($taskInfo.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Reparse path rejected: $taskCheck" }
        } elseif (-not $AllowMissing) { throw "Missing path: $taskCheck" }
        $taskCheck = [IO.Path]::GetDirectoryName($taskCheck)
    }
}

function Get-Sha([IO.FileStream]$Stream) {
    $Stream.Position = 0
    $taskHasher = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($taskHasher.ComputeHash($Stream)).Replace('-', '') }
    finally { $taskHasher.Dispose(); $Stream.Position = 0 }
}

function Add-Input([string]$Path, [string]$Expected = '') {
    $taskPath = [IO.Path]::GetFullPath($Path)
    if ($taskInputByPath.ContainsKey($taskPath)) {
        $taskPrior = $taskInputByPath[$taskPath]
        if ($Expected -and $taskPrior.sha256 -cne $Expected.ToUpperInvariant()) { throw "Input pin differs: $taskPath" }
        return $taskPrior
    }
    $taskStream = $taskGuards.OpenRead($taskPath)
    $taskStreams.Add($taskStream)
    $taskRecord = [pscustomobject]@{ path = $taskPath; bytes = $taskStream.Length; sha256 = (Get-Sha $taskStream); identity = [TokenMeowInstallerBuildGuards]::Identity($taskStream) }
    $taskInputs.Add($taskRecord)
    $taskInputByPath.Add($taskPath, $taskRecord)
    if ($Expected -and $taskRecord.sha256 -cne $Expected.ToUpperInvariant()) { throw "Input SHA differs: $taskPath" }
    return $taskRecord
}

function Read-SmallJson([string]$Path) {
    $taskMeta = Add-Input $Path
    if ($taskMeta.bytes -gt 4MB) { throw "JSON exceeds the fixed 4 MiB bound: $Path" }
    return Get-Content -LiteralPath $taskMeta.path -Raw -Encoding utf8 | ConvertFrom-Json
}

function Write-NewText([string]$Path, [string]$Text) {
    $taskBytes = [Text.UTF8Encoding]::new($false).GetBytes($Text)
    $taskWriter = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
    try { $taskWriter.Write($taskBytes, 0, $taskBytes.Length) } finally { $taskWriter.Dispose() }
}

function Invoke-Tool([string]$Executable, [string[]]$Arguments, [string]$Name, [hashtable]$EnvironmentOverrides = @{}, [string]$BinaryOutput = '') {
    $null = Add-Input $Executable
    $taskInfo = [Diagnostics.ProcessStartInfo]::new()
    $taskInfo.FileName = [IO.Path]::GetFullPath($Executable)
    $taskInfo.WorkingDirectory = $taskRoot
    $taskInfo.UseShellExecute = $false
    $taskInfo.CreateNoWindow = $true
    $taskInfo.RedirectStandardOutput = $true
    $taskInfo.RedirectStandardError = $true
    foreach ($taskArgument in $Arguments) { $taskInfo.ArgumentList.Add($taskArgument) }
    foreach ($taskKey in $EnvironmentOverrides.Keys) {
        if ($null -eq $EnvironmentOverrides[$taskKey]) { $null = $taskInfo.Environment.Remove($taskKey) }
        else { $taskInfo.Environment[$taskKey] = [string]$EnvironmentOverrides[$taskKey] }
    }
    $taskStdout = if ($BinaryOutput) { $BinaryOutput } else { Join-Path $taskWork "$Name.stdout.log" }
    $taskStderr = Join-Path $taskWork "$Name.stderr.log"
    $taskOut = $null
    $taskErr = $null
    $taskProcess = [Diagnostics.Process]::new()
    $taskProcess.StartInfo = $taskInfo
    $taskStartedProcess = $false
    $taskCopyOut = $null
    $taskCopyErr = $null
    $taskRecord = [ordered]@{ name = $Name; executable = $taskInfo.FileName; arguments = $Arguments; environmentOverrides = $EnvironmentOverrides; stdout = $taskStdout; stderr = $taskStderr; startedAtUtc = [DateTimeOffset]::UtcNow.ToString('O') }
    try {
        $taskOut = [IO.File]::Open($taskStdout, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
        $taskErr = [IO.File]::Open($taskStderr, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
        if (-not $taskProcess.Start()) { throw "Cannot start build tool: $Name" }
        $taskStartedProcess = $true
        $taskCopyOut = [TokenMeowInstallerBuildGuards]::CapturePipe($taskProcess.StandardOutput.BaseStream, $taskOut)
        $taskCopyErr = [TokenMeowInstallerBuildGuards]::CapturePipe($taskProcess.StandardError.BaseStream, $taskErr)
        $taskRecord.processId = $taskProcess.Id
        $taskRecord.creationUtc = $taskProcess.StartTime.ToUniversalTime().ToString('O')
        $taskProcess.WaitForExit()
        $taskPipeErrors = @($taskCopyOut.GetAwaiter().GetResult(), $taskCopyErr.GetAwaiter().GetResult()) | Where-Object { $_ }
        $taskRecord.exitCode = $taskProcess.ExitCode
        $taskRecord.pipeErrors = @($taskPipeErrors)
        $taskRecord.completedAtUtc = [DateTimeOffset]::UtcNow.ToString('O')
        $taskCommands.Add([pscustomobject]$taskRecord)
        if ($taskPipeErrors) { throw "Build tool $Name output capture failed: $($taskPipeErrors -join '; ')" }
        if ($taskProcess.ExitCode -ne 0) { throw "Build tool $Name exited $($taskProcess.ExitCode); preserve $taskStderr" }
    } finally {
        if ($taskStartedProcess) {
            # Finish draining any child already started before releasing input or
            # output handles, including metadata/capture-error paths. No Kill.
            if (-not $taskProcess.HasExited) { $taskProcess.WaitForExit() }
            if ($null -ne $taskCopyOut) { $null = $taskCopyOut.GetAwaiter().GetResult() }
            if ($null -ne $taskCopyErr) { $null = $taskCopyErr.GetAwaiter().GetResult() }
        }
        try { if ($null -ne $taskOut) { $taskOut.Dispose() } }
        finally {
            try { if ($null -ne $taskErr) { $taskErr.Dispose() } }
            finally { $taskProcess.Dispose() }
        }
    }
}

function Copy-NewFile([string]$Source, [string]$Destination) {
    $taskSource = $null
    $taskDestination = $null
    try {
        $taskSource = [IO.File]::Open($Source, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        $taskDestination = [IO.File]::Open($Destination, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
        $taskSource.CopyTo($taskDestination, 65536)
    } finally {
        try { if ($null -ne $taskDestination) { $taskDestination.Dispose() } }
        finally { if ($null -ne $taskSource) { $taskSource.Dispose() } }
    }
}

function Assert-App([string]$Path, [string]$Version, [string]$Product) {
    $taskApp = Add-Input $Path
    $taskReader = [IO.BinaryReader]::new([IO.File]::Open($Path, 'Open', 'Read', 'Read'))
    try {
        if ($taskReader.ReadUInt16() -ne 0x5A4D) { throw 'Application has no MZ header.' }
        $taskReader.BaseStream.Position = 0x3c
        $taskPe = $taskReader.ReadUInt32()
        if ($taskPe -gt $taskApp.bytes - 26) { throw 'Application PE offset is invalid.' }
        $taskReader.BaseStream.Position = $taskPe
        if ($taskReader.ReadUInt32() -ne 0x4550 -or $taskReader.ReadUInt16() -ne 0x8664) { throw 'Application is not AMD64 PE.' }
        $taskReader.BaseStream.Position = $taskPe + 24
        if ($taskReader.ReadUInt16() -ne 0x20b) { throw 'Application is not PE32+.' }
    } finally { $taskReader.Dispose() }
    $taskVersionInfo = [Diagnostics.FileVersionInfo]::GetVersionInfo($Path)
    if ($taskVersionInfo.ProductVersion -cne $Version -or $taskVersionInfo.ProductName -cne $Product) { throw 'Extracted application product/version differs from current configuration.' }
    return $taskApp
}

try {
    if (-not $IsWindows -or -not [Environment]::Is64BitOperatingSystem) { throw 'PowerShell 7 on Windows x64 is required.' }
    if ($Mode -eq 'ReuseEngine' -and (-not $EnginePath -or -not $EngineExpectedSha256)) { throw 'ReuseEngine requires an explicit EnginePath and EngineExpectedSha256.' }
    if ($Mode -eq 'Build' -and ($EnginePath -or $EngineExpectedSha256)) { throw 'Engine pins are accepted only in ReuseEngine mode.' }
    if ($PreflightOnly -and $Mode -ne 'Build') { throw 'PreflightOnly is the read-only compiler-resolution check for Build mode.' }
    if (($SevenZipDllPath -and -not $SevenZipDllExpectedSha256) -or ($SevenZipDllExpectedSha256 -and -not $SevenZipDllPath)) { throw 'Pin both the 7z DLL path and its SHA, or omit both.' }
    $taskSevenZip = [IO.Path]::GetFullPath($SevenZipPath)
    Assert-NoReparse $taskSevenZip
    $taskAdjacentDll = Join-Path ([IO.Path]::GetDirectoryName($taskSevenZip)) '7z.dll'
    if (Test-Path -LiteralPath $taskAdjacentDll) {
        if (-not $SevenZipDllPath -or -not [string]::Equals([IO.Path]::GetFullPath($SevenZipDllPath), $taskAdjacentDll, [StringComparison]::OrdinalIgnoreCase)) { throw 'The actual adjacent 7z.dll must be explicitly pinned.' }
    } elseif ($SevenZipDllPath) { throw 'The pinned 7z.dll must be adjacent to the extractor.' }
    $taskGuardSource = Join-Path $PSScriptRoot 'windows-installer-build-guards.cs'
    Assert-NoReparse $taskGuardSource
    $taskBootstrap = [IO.File]::Open($taskGuardSource, 'Open', 'Read', 'Read')
    try { Add-Type -Path $taskGuardSource } finally { $taskBootstrap.Dispose() }
    $taskGuards = [TokenMeowInstallerBuildGuards]::new()
    $null = Add-Input $PSCommandPath
    $null = Add-Input $taskGuardSource
    $taskTool = Add-Input $taskSevenZip $SevenZipExpectedSha256
    $taskDll = if ($SevenZipDllPath) { Add-Input $SevenZipDllPath $SevenZipDllExpectedSha256 } else { $null }
    $taskConfig = Read-SmallJson (Join-Path $taskRoot 'src-tauri\tauri.conf.json')
    $taskPackage = Read-SmallJson (Join-Path $taskRoot 'package.json')
    $taskCargoVersion = [regex]::Match([IO.File]::ReadAllText((Join-Path $taskRoot 'src-tauri/Cargo.toml')), '(?m)^version = "([^"]+)"').Groups[1].Value
    $taskLockVersion = [regex]::Match([IO.File]::ReadAllText((Join-Path $taskRoot 'src-tauri/Cargo.lock')), 'name = "token-meow"\r?\nversion = "([^"]+)"').Groups[1].Value
    if ($taskPackage.version -cne $taskConfig.version -or $taskCargoVersion -cne $taskConfig.version -or $taskLockVersion -cne $taskConfig.version) { throw 'Release versions differ; run node scripts/release-version.mjs --set <version>.' }
    if ($taskConfig.version -notmatch '^\d+\.\d+\.\d+$' -or $taskConfig.productName -cne 'TokenMeow') { throw 'Review the fixed product/version contract before building another product.' }
    if (-not $OutputDirectory) { $OutputDirectory = Join-Path $taskOutputRoot ('windows-installer-' + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ') + '-' + [Guid]::NewGuid().ToString('N')) }
    $taskRun = [IO.Path]::GetFullPath($OutputDirectory)
    if (-not $taskRun.StartsWith($taskOutputRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Run output must be a new directory within this workspace output/.' }
    Assert-NoReparse $taskRun -AllowMissing
    if (Test-Path -LiteralPath $taskRun) { throw 'Output already exists. Preserve it and select a new directory.' }
    if (-not (Test-Path -LiteralPath ([IO.Path]::GetDirectoryName($taskRun)) -PathType Container)) { throw 'Output parent must already exist.' }
    $taskWork = Join-Path $taskRun 'work'
    $taskRelease = Join-Path $taskRun 'release'
    [TokenMeowInstallerBuildGuards]::CreateNewDirectory($taskRun)
    $taskCreated = $true
    $taskGuards.HoldFolder($taskRun)
    [TokenMeowInstallerBuildGuards]::CreateNewDirectory($taskWork)
    [TokenMeowInstallerBuildGuards]::CreateNewDirectory($taskRelease)
    $taskGuards.HoldFolder($taskWork)
    $taskGuards.HoldFolder($taskRelease)
    $taskStarted = [DateTimeOffset]::UtcNow.ToString('O')
    $taskSourcePins = @('src-tauri/windows/installer.nsi', 'src-tauri/windows/installer-hooks.nsh', 'src-tauri/windows/installer-engine.nsh')
    foreach ($taskRelative in $taskSourcePins) { $null = Add-Input (Join-Path $taskRoot $taskRelative) }
    $taskBuilder = Join-Path $PSScriptRoot 'build-custom-installer.ps1'
    $null = Add-Input $taskBuilder
    $null = Add-Input (Join-Path $taskRoot 'package.json')
    foreach ($taskFile in Get-ChildItem -LiteralPath (Join-Path $taskRoot 'installer\TokenMeow.Setup') -File -Recurse) {
        if ($taskFile.FullName -notmatch '[\\/]tests[\\/]') { $null = Add-Input $taskFile.FullName }
    }
    if ($Mode -eq 'Build') {
        if (-not $CargoTargetDirectory) { $CargoTargetDirectory = Join-Path $taskWork 'cargo-target' }
        $taskTarget = [IO.Path]::GetFullPath($CargoTargetDirectory)
        if (-not $taskTarget.StartsWith($taskOutputRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Cargo target must be isolated within this project output/.' }
        Assert-NoReparse $taskTarget -AllowMissing
        if (Test-Path -LiteralPath $taskTarget) { throw 'Cargo target must be new, never a shared or previous target.' }
        if (-not (Test-Path -LiteralPath ([IO.Path]::GetDirectoryName($taskTarget)) -PathType Container)) { throw 'Cargo target parent must already exist.' }
        [TokenMeowInstallerBuildGuards]::CreateNewDirectory($taskTarget)
        $taskGuards.HoldFolder($taskTarget)
        $taskNode = (Get-Command node.exe -CommandType Application | Select-Object -First 1).Source
        $null = Add-Input $taskNode
        $taskCliAlias = Join-Path $taskRoot 'node_modules\@tauri-apps\cli\tauri.js'
        $taskPnpmStore = Join-Path $taskRoot 'node_modules\.pnpm'
        Assert-NoReparse $taskPnpmStore
        $taskResolver = @'
const fs = require('node:fs');
const path = require('node:path');
const { createRequire } = require('node:module');
const store = fs.realpathSync(process.argv[2]);
function inside(p) {
  const rel = path.relative(store, p);
  if (!rel || rel.startsWith('..' + path.sep) || rel === '..' || path.isAbsolute(rel)) throw Error('CLI escaped the local pnpm store');
  return p;
}
const entry = inside(fs.realpathSync(process.argv[1]));
const wrapperRoot = path.dirname(entry);
const packageJson = inside(fs.realpathSync(path.join(wrapperRoot, 'package.json')));
const wrapper = JSON.parse(fs.readFileSync(packageJson, 'utf8'));
if (wrapper.name !== '@tauri-apps/cli' || wrapper.version !== '2.12.3' || path.basename(entry) !== 'tauri.js') throw Error('Unreviewed Tauri CLI wrapper');
if (process.platform !== 'win32' || process.arch !== 'x64') throw Error('Windows x64 Node required');
const resolveOnly = createRequire(packageJson);
const nativeEntry = inside(fs.realpathSync(resolveOnly.resolve('@tauri-apps/cli-win32-x64-msvc')));
const nativeRoot = path.dirname(nativeEntry);
const nativePackageJson = inside(fs.realpathSync(path.join(nativeRoot, 'package.json')));
const native = JSON.parse(fs.readFileSync(nativePackageJson, 'utf8'));
if (native.name !== '@tauri-apps/cli-win32-x64-msvc' || native.version !== '2.12.3' || path.extname(nativeEntry) !== '.node') throw Error('Unreviewed native Tauri CLI package');
console.log(JSON.stringify({store, wrapper:{entry,root:wrapperRoot,packageJson,name:wrapper.name,version:wrapper.version}, native:{entry:nativeEntry,root:nativeRoot,packageJson:nativePackageJson,name:native.name,version:native.version}, packageCodeExecuted:false}));
'@
        Invoke-Tool $taskNode @('-e', $taskResolver, $taskCliAlias, $taskPnpmStore) 'resolve-tauri' @{ NODE_OPTIONS = $null; NODE_PATH = $null; NAPI_RS_NATIVE_LIBRARY_PATH = $null }
        $taskCliBinding = Read-SmallJson (Join-Path $taskWork 'resolve-tauri.stdout.log')
        foreach ($taskPackage in @($taskCliBinding.wrapper, $taskCliBinding.native)) {
            $taskCanonicalRoot = [IO.Path]::GetFullPath($taskPackage.root)
            if (-not $taskCanonicalRoot.StartsWith($taskPnpmStore + '\', [StringComparison]::OrdinalIgnoreCase) -or [string]$taskPackage.version -cne '2.12.3') { throw 'Resolved CLI package is outside the reviewed local pnpm store/version.' }
            Assert-NoReparse $taskCanonicalRoot
            $taskMetadata = Read-SmallJson $taskPackage.packageJson
            if ($taskMetadata.name -cne $taskPackage.name -or $taskMetadata.version -cne '2.12.3') { throw 'Held canonical CLI package metadata differs.' }
            foreach ($taskFile in Get-ChildItem -LiteralPath $taskCanonicalRoot -File -Recurse) { $null = Add-Input $taskFile.FullName }
        }
        $taskCli = [IO.Path]::GetFullPath($taskCliBinding.wrapper.entry)
        $null = Add-Input $taskCli
        $null = Add-Input $taskCliBinding.native.entry
        if ($PreflightOnly) {
            foreach ($taskIndex in 0..($taskInputs.Count - 1)) {
                if ((Get-Sha $taskStreams[$taskIndex]) -cne $taskInputs[$taskIndex].sha256 -or [TokenMeowInstallerBuildGuards]::Identity($taskStreams[$taskIndex]) -cne $taskInputs[$taskIndex].identity) { throw 'CLI preflight input changed.' }
            }
            $taskPreflight = [ordered]@{ status = 'CANONICAL_PINNED_CLI_PREFLIGHT_ONLY_NO_CARGO_OR_INSTALLER'; cli = $taskCliBinding; inputs = $taskInputs.ToArray(); commands = $taskCommands.ToArray(); inputPostHashesAndIdentitiesMatched = $true; inputHandlesReleasedByFinally = $true; fullApplicationSourcesAndAssetsNotYetValidated = $true; cargoExecuted = $false; installerExecuted = $false; distributableCreated = $false }
            Write-NewText (Join-Path $taskWork 'cli-preflight.json') (($taskPreflight | ConvertTo-Json -Depth 10) + "`n")
            [pscustomobject]@{ status = $taskPreflight.status; receipt = (Join-Path $taskWork 'cli-preflight.json'); distributableCreated = $false } | ConvertTo-Json
            return
        }
        foreach ($taskRelative in @('pnpm-lock.yaml', 'index.html', 'vite.config.ts', 'tsconfig.json', 'src-tauri/build.rs', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock', 'scripts/distribution-manifest.mjs', 'release/update-public-key.pub')) { $null = Add-Input (Join-Path $taskRoot $taskRelative) }
        foreach ($taskFolder in @('src', 'src-tauri/src', 'src-tauri/migrations', 'src-tauri/icons', 'src-tauri/capabilities', 'third-party')) {
            foreach ($taskFile in Get-ChildItem -LiteralPath (Join-Path $taskRoot $taskFolder) -File -Recurse) { $null = Add-Input $taskFile.FullName }
        }
        foreach ($taskRelative in @('rust-toolchain.toml', '.cargo/config.toml', 'src-tauri/.cargo/config.toml')) {
            $taskOptional = Join-Path $taskRoot $taskRelative
            if (Test-Path -LiteralPath $taskOptional) { $null = Add-Input $taskOptional }
        }
        $taskAsset = Read-SmallJson (Join-Path $taskRoot 'assets/pets/nekomi/manifest-animation-v12.json')
        $taskAssetBindings = @($taskAsset.frames) + @($taskAsset.source_archive.manifest) + @($taskAsset.source_archive.generation)
        foreach ($taskAssetBinding in $taskAssetBindings) {
            if ([string]$taskAssetBinding.sha256 -notmatch '^[a-fA-F0-9]{64}$') { throw 'Asset/source SHA pin is missing.' }
            $taskAssetPath = [IO.Path]::GetFullPath((Join-Path $taskRoot $taskAssetBinding.path))
            if (-not $taskAssetPath.StartsWith($taskRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Asset/source binding escaped the project.' }
            $null = Add-Input $taskAssetPath $taskAssetBinding.sha256
        }
        $null = Add-Input $taskCli
        $taskEnvironment = @{ CARGO_TARGET_DIR = $taskTarget; CARGO_BUILD_JOBS = '1'; CARGO_BUILD_TARGET = $null; RUSTFLAGS = $null; CARGO_ENCODED_RUSTFLAGS = $null; RUSTC_WRAPPER = $null; RUSTC_WORKSPACE_WRAPPER = $null; TAURI_CONFIG = $null; NODE_OPTIONS = $null; NODE_PATH = $null; NAPI_RS_NATIVE_LIBRARY_PATH = $taskCliBinding.native.entry; NAPI_RS_ENFORCE_VERSION_CHECK = $null }
        foreach ($taskKey in [Environment]::GetEnvironmentVariables().Keys) {
            if ([string]$taskKey -like 'CARGO_PROFILE_*') { $taskEnvironment[[string]$taskKey] = $null }
        }
        Invoke-Tool $taskNode @((Join-Path $taskRoot 'scripts\distribution-manifest.mjs')) 'legal' $taskEnvironment
        foreach ($taskFile in Get-ChildItem -LiteralPath (Join-Path $taskOutputRoot 'distribution-licenses') -File -Recurse) { $null = Add-Input $taskFile.FullName }
        Invoke-Tool $taskNode @($taskCli, 'build', '--ci', '--target', 'x86_64-pc-windows-msvc', '--bundles', 'nsis', '--', '--locked', '--offline', '--jobs', '1', '--config', 'profile.release.package.token-meow.codegen-units=16') 'tauri' $taskEnvironment
        $taskBundle = Join-Path $taskTarget 'x86_64-pc-windows-msvc\release\bundle\nsis'
        $taskEngines = @(Get-ChildItem -LiteralPath $taskBundle -File -Filter '*.exe')
        if ($taskEngines.Count -ne 1) { throw 'Expected exactly one freshly built NSIS engine.' }
        $taskEngine = Add-Input $taskEngines[0].FullName
    } else {
        if ($CargoTargetDirectory) { throw 'ReuseEngine does not use a Cargo target.' }
        $taskEngine = Add-Input $EnginePath $EngineExpectedSha256
        $taskTarget = $null
    }
    Invoke-Tool $taskSevenZip @('l', '-slt', '-bd', '-sccUTF-8', '--', $taskEngine.path) 'engine-list'
    $taskListPath = Join-Path $taskWork 'engine-list.stdout.log'
    if ((Get-Item -LiteralPath $taskListPath).Length -gt 4MB) { throw '7z listing exceeds the fixed bound.' }
    $taskEntries = [Collections.Generic.List[object]]::new()
    $taskHeaders = [Collections.Generic.List[object]]::new()
    foreach ($taskBlock in ([IO.File]::ReadAllText($taskListPath) -split '\r?\n\r?\n')) {
        $taskFields = @{}
        foreach ($taskLine in ($taskBlock -split '\r?\n')) {
            if ($taskLine -match '^([^=]+?) = (.*)$') { $taskFields[$Matches[1]] = $Matches[2] }
        }
        if ($taskFields.ContainsKey('Type')) { $taskHeaders.Add($taskFields) }
        if ($taskFields.ContainsKey('Path') -and $taskFields.ContainsKey('Size')) { $taskEntries.Add($taskFields) }
    }
    if ($taskHeaders.Count -ne 1 -or $taskHeaders[0].Type -cne 'Nsis' -or -not [string]::Equals([IO.Path]::GetFullPath($taskHeaders[0].Path), $taskEngine.path, [StringComparison]::OrdinalIgnoreCase) -or [long]$taskHeaders[0]['Physical Size'] -ne $taskEngine.bytes) { throw 'The pinned input must be one exact NSIS archive with matching physical size.' }
    $taskExtracted = @{}
    foreach ($taskEntryName in @('token-meow.exe', 'licenses/manifest.json')) {
        $taskMatches = @($taskEntries | Where-Object { $_.Path.Replace('\', '/') -ceq $taskEntryName })
        if ($taskMatches.Count -ne 1 -or ($taskMatches[0].ContainsKey('Folder') -and $taskMatches[0].Folder -eq '+')) { throw "Expected one exact ordinary payload entry: $taskEntryName" }
        $taskExpectedBytes = [long]::Parse($taskMatches[0].Size, [Globalization.CultureInfo]::InvariantCulture)
        if ($taskExpectedBytes -le 0 -or $taskExpectedBytes -gt 1GB -or ($taskEntryName -eq 'licenses/manifest.json' -and $taskExpectedBytes -gt 4MB)) { throw 'Payload size exceeds extraction bounds.' }
        $taskName = if ($taskEntryName -eq 'token-meow.exe') { 'application' } else { 'legal-manifest' }
        $taskDestination = Join-Path $taskWork ($taskName + $(if ($taskName -eq 'application') { '.exe' } else { '.json' }))
        Invoke-Tool $taskSevenZip @('x', '-so', '-bd', '-bb0', '-sccUTF-8', '--', $taskEngine.path, $taskMatches[0].Path) $taskName @{} $taskDestination
        $taskPayload = Add-Input $taskDestination
        if ($taskPayload.bytes -ne $taskExpectedBytes) { throw 'Actual payload length differs from the unique archive entry.' }
        $taskExtracted[$taskEntryName] = $taskPayload
    }
    $taskApp = Assert-App $taskExtracted['token-meow.exe'].path $taskConfig.version $taskConfig.productName
    $taskLegal = Read-SmallJson $taskExtracted['licenses/manifest.json'].path
    if ($taskLegal.product -cne $taskConfig.productName -or $taskLegal.version -cne $taskConfig.version) { throw 'Embedded legal manifest product/version differs.' }
    $taskLocal = @($taskLegal.installerTools | Where-Object { $_.PSObject.Properties.Name -contains 'localSource' })
    if ($taskLocal.Count -ne 1 -or $taskLocal[0].localSource -cne $taskSourcePins[0]) { throw 'Expected exactly one current local NSIS template provenance record.' }
    if ([string]$taskLocal[0].localSha256 -notmatch '^[a-fA-F0-9]{64}$') { throw 'Embedded legal template SHA pin is missing.' }
    $taskActualTemplate = Add-Input (Join-Path $taskRoot $taskSourcePins[0]) $taskLocal[0].localSha256
    $taskSupporting = @($taskLocal[0].localSupportingSources)
    if ($taskSupporting.Count -ne 2) { throw 'Expected exactly two supporting NSIS sources.' }
    foreach ($taskRelative in $taskSourcePins[1..2]) {
        $taskMatch = @($taskSupporting | Where-Object { $_.source -ceq $taskRelative })
        if ($taskMatch.Count -ne 1) { throw 'Embedded legal supporting source set differs.' }
        if ([string]$taskMatch[0].sha256 -notmatch '^[a-fA-F0-9]{64}$') { throw 'Embedded legal supporting source SHA pin is missing.' }
        $taskSupport = Add-Input (Join-Path $taskRoot $taskRelative) $taskMatch[0].sha256
        if ($taskSupport.bytes -ne $taskMatch[0].bytes) { throw 'Supporting source length differs.' }
    }
    $taskPwsh = (Get-Process -Id $PID).Path
    $taskOuter = Join-Path $taskWork 'outer'
    $taskPayloadFiles = @($taskEntries | Where-Object { $_.Path -and (-not $_.ContainsKey('Folder') -or $_.Folder -ne '+') } | ForEach-Object { $_.Path.Replace('\', '/') } | Where-Object { $_ -ceq 'token-meow.exe' -or $_.StartsWith('licenses/', [StringComparison]::Ordinal) })
    $taskPayloadFilesPath = Join-Path $taskWork 'payload-files.json'
    Write-NewText $taskPayloadFilesPath (($taskPayloadFiles | ConvertTo-Json) + "`n")
    $null = Add-Input $taskPayloadFilesPath
    Invoke-Tool $taskPwsh @('-NoProfile', '-NonInteractive', '-File', $taskBuilder, '-EnginePath', $taskEngine.path, '-EngineExpectedSha256', $taskEngine.sha256, '-ApplicationPath', $taskApp.path, '-ApplicationExpectedSha256', $taskApp.sha256, '-PayloadFilesPath', $taskPayloadFilesPath, '-OutputDirectory', $taskOuter) 'custom-shell'
    $taskOuterReceipt = Read-SmallJson (Join-Path $taskOuter 'build-receipt.json')
    if ($taskOuterReceipt.status -cne 'CUSTOM_INSTALLER_COMPILED_NOT_INSTALLED' -or $taskOuterReceipt.compilerExitCode -ne 0 -or -not $taskOuterReceipt.heldInputsPostHashMatched -or -not $taskOuterReceipt.allInputHandlesClosed -or $taskOuterReceipt.engine.sha256 -cne $taskEngine.sha256 -or $taskOuterReceipt.application.sha256 -cne $taskApp.sha256) { throw 'Custom shell build receipt binding differs.' }
    $taskExpectedShell = Join-Path $taskOuter "TokenMeow_$($taskConfig.version)_x64-custom-setup.exe"
    if (-not [string]::Equals([IO.Path]::GetFullPath($taskOuterReceipt.executable.path), $taskExpectedShell, [StringComparison]::OrdinalIgnoreCase) -or [string]$taskOuterReceipt.executable.sha256 -notmatch '^[a-fA-F0-9]{64}$') { throw 'Shell receipt path or SHA is invalid.' }
    $taskShell = Add-Input $taskExpectedShell $taskOuterReceipt.executable.sha256
    if ($taskShell.bytes -ne $taskOuterReceipt.executable.bytes) { throw 'Shell receipt byte length differs.' }
    $taskReleaseSetup = Join-Path $taskRelease ([IO.Path]::GetFileName($taskShell.path))
    Copy-NewFile $taskShell.path $taskReleaseSetup
    $taskReleased = Add-Input $taskReleaseSetup $taskShell.sha256
    foreach ($taskIndex in 0..($taskInputs.Count - 1)) {
        if ((Get-Sha $taskStreams[$taskIndex]) -cne $taskInputs[$taskIndex].sha256 -or [TokenMeowInstallerBuildGuards]::Identity($taskStreams[$taskIndex]) -cne $taskInputs[$taskIndex].identity) { throw 'A held input changed during orchestration.' }
    }
    $taskReceipt = [ordered]@{
        status = 'BRANDED_WINDOWS_INSTALLER_BUILT_AND_STATICALLY_BOUND_NOT_INSTALLED'
        mode = $Mode; startedAtUtc = $taskStarted; completedAtUtc = [DateTimeOffset]::UtcNow.ToString('O')
        product = $taskConfig.productName; productVersion = $taskConfig.version
        releaseDirectory = $taskRelease; executable = $taskReleased
        engine = $taskEngine; archiveHeader = $taskHeaders[0]; actualEnginePayload = $taskApp; embeddedLegalManifest = $taskExtracted['licenses/manifest.json']
        nsisTemplateAndTwoSupportingSourcesMatchCurrent = $true
        extractor = $taskTool; extractorDll = $taskDll; cargoTargetDirectory = $taskTarget; canonicalCli = $taskCliBinding
        originalBundleNsisIsInternalEngineNotDistributable = $true
        applicationRecompiledByThisRun = ($Mode -eq 'Build'); applicationPayloadManuallyPatchedByWrapper = $false
        commands = $taskCommands.ToArray(); heldInputs = $taskInputs.ToArray()
        heldInputPostHashesAndIdentitiesMatched = $true; allInputHandlesClosed = $false
        finalOutputDirectoryGuardHeldAtReceiptWrite = $true
        inputClosureExcludesFinalOutputDirectoryGuard = $true
        installerExecuted = $false; applicationExecuted = $false; upgradeQualified = $false; uninstallQualified = $false; wholeV1Qualified = $false; codeSignedByThisRun = ((Get-AuthenticodeSignature -LiteralPath $taskReleaseSetup).Status -eq 'Valid')
        limitations = @('Compilation and extraction do not validate installation, upgrade, WebView2 download, shortcuts, launch or uninstall.', 'Tools are observed by retained Process objects; this script does not claim whole-Job lifecycle or Native qualification.', 'Only release/ is distributable. work/, Cargo bundle/nsis and all raw engine EXEs are internal build inputs.')
    }
    # Transfer output-directory protection before closing the input guard set.
    # The receipt's input closure does not claim this finalization guard is closed.
    $taskFinalizationGuards = [TokenMeowInstallerBuildGuards]::new()
    $taskFinalizationGuards.HoldFolder($taskRun)
    $taskFinalizationGuards.HoldFolder($taskRelease)
    foreach ($taskStream in $taskStreams) { $taskStream.Dispose() }
    $taskGuards.Dispose()
    $taskClosed = $true
    $taskReceipt.allInputHandlesClosed = $true
    Write-NewText (Join-Path $taskRelease 'receipt.json') (($taskReceipt | ConvertTo-Json -Depth 12) + "`n")
    $taskReceiptSha = (Get-FileHash -LiteralPath (Join-Path $taskRelease 'receipt.json') -Algorithm SHA256).Hash
    Write-NewText (Join-Path $taskRelease 'SHA256SUMS.txt') ($taskShell.sha256 + '  ' + [IO.Path]::GetFileName($taskReleaseSetup) + "`n" + $taskReceiptSha + "  receipt.json`n")
    if (@(Get-ChildItem -LiteralPath $taskRelease -File).Count -ne 3) { throw 'Release directory must contain only branded setup, SHA256SUMS.txt and receipt.json.' }
    [pscustomobject]@{ status = $taskReceipt.status; setup = $taskReleaseSetup; sha256 = $taskShell.sha256; receiptSha256 = $taskReceiptSha; mode = $Mode; installed = $false } | ConvertTo-Json
} catch {
    if ($taskCreated) {
        $taskFailure = Join-Path $taskRun 'failure.json'
        if (-not (Test-Path -LiteralPath $taskFailure)) { Write-NewText $taskFailure (([ordered]@{ status = 'FAILED_PRESERVED'; error = $_.Exception.Message; commands = $taskCommands.ToArray(); installerOrApplicationExecuted = $false } | ConvertTo-Json -Depth 10) + "`n") }
    }
    throw
} finally {
    try {
        if (-not $taskClosed) {
            try { foreach ($taskStream in $taskStreams) { $taskStream.Dispose() } }
            finally { if ($null -ne $taskGuards) { $taskGuards.Dispose() } }
        }
    } finally {
        if ($null -ne $taskFinalizationGuards) { $taskFinalizationGuards.Dispose() }
    }
}
