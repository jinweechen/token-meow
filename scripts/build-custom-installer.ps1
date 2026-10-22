param(
    [Parameter(Mandatory)][string]$EnginePath,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$EngineExpectedSha256,
    [Parameter(Mandatory)][string]$ApplicationPath,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$ApplicationExpectedSha256,
    [Parameter(Mandatory)][string]$PayloadFilesPath,
    [Parameter(Mandatory)][string]$OutputDirectory
)

# Build only. This script never executes the setup, engine, application or uninstaller.
$ErrorActionPreference = 'Stop'
$taskRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$taskSource = Join-Path $taskRoot 'installer\TokenMeow.Setup'
$taskOutput = [IO.Path]::GetFullPath($OutputDirectory)
$taskFramework = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319'
$taskCompiler = Join-Path $taskFramework 'csc.exe'
$taskManifest = Join-Path $taskSource 'app.manifest'
$taskHero = Join-Path $taskRoot 'assets\pets\nekomi\walk-v10-complete\model-reference\master-front-v2.png'
$taskIcon = Join-Path $taskRoot 'src-tauri\icons\icon.ico'
$taskConfiguration = Join-Path $taskRoot 'src-tauri\tauri.conf.json'
$taskHeroSha = '7EC89E91A67884A6E524B788E328A763AAEBE502C2A5B5C887B5679FD7620997'
$taskInputs = [Collections.Generic.List[object]]::new()
$taskStreams = [Collections.Generic.List[IO.FileStream]]::new()
$taskClosed = $false

function Get-StreamSha([IO.FileStream]$Stream) {
    $Stream.Position = 0
    $taskHasher = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($taskHasher.ComputeHash($Stream)).Replace('-', '') }
    finally { $taskHasher.Dispose(); $Stream.Position = 0 }
}

function Add-Input([string]$Path, [string]$Expected = '') {
    $taskFullPath = [IO.Path]::GetFullPath($Path)
    $taskItem = Get-Item -LiteralPath $taskFullPath
    if ($taskItem.PSIsContainer -or ($taskItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "Expected an ordinary input file: $taskFullPath"
    }
    $taskHandle = [IO.File]::Open($taskFullPath, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $taskStreams.Add($taskHandle)
    $taskDigest = Get-StreamSha $taskHandle
    if ($Expected -and $taskDigest -cne $Expected.ToUpperInvariant()) { throw "Input SHA differs: $taskFullPath" }
    $taskRecord = [pscustomobject]@{ path = $taskFullPath; bytes = $taskHandle.Length; sha256 = $taskDigest }
    $taskInputs.Add($taskRecord)
    return $taskRecord
}

function Write-NewUtf8([string]$Path, [string]$Text) {
    $taskWriter = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
    try { $taskEncoded = [Text.UTF8Encoding]::new($false).GetBytes($Text); $taskWriter.Write($taskEncoded, 0, $taskEncoded.Length) }
    finally { $taskWriter.Dispose() }
}

function Escape-CSharp([string]$Text) {
    return $Text.Replace('\', '\\').Replace('"', '\"').Replace("`r", '\r').Replace("`n", '\n')
}

try {
    if (-not [Environment]::Is64BitOperatingSystem) { throw 'Windows x64 is required.' }
    if (Test-Path -LiteralPath $taskOutput) { throw 'Output already exists; preserve the previous attempt and choose a new directory.' }
    if (-not (Test-Path -LiteralPath (Split-Path -Parent $taskOutput) -PathType Container)) { throw 'The output parent must exist.' }
    $taskEngine = Add-Input $EnginePath $EngineExpectedSha256
    $taskApplication = Add-Input $ApplicationPath $ApplicationExpectedSha256
    $null = Add-Input $PayloadFilesPath
    $taskPayloadFiles = @(Get-Content -LiteralPath $PayloadFilesPath -Raw | ConvertFrom-Json)
    if ($taskPayloadFiles.Count -lt 2 -or $taskPayloadFiles.Count -gt 8192 -or 'token-meow.exe' -cnotin $taskPayloadFiles -or 'licenses/manifest.json' -cnotin $taskPayloadFiles) { throw 'The actual engine payload file list is incomplete.' }
    foreach ($taskPayloadName in $taskPayloadFiles) {
        if ($taskPayloadName -cne 'token-meow.exe' -and $taskPayloadName -notmatch '^licenses/[^:]+$') { throw 'Unsupported payload destination.' }
        foreach ($taskPart in $taskPayloadName.Split('/')) { if ($taskPart -in @('', '.', '..') -or $taskPart -ne $taskPart.Trim() -or $taskPart.EndsWith('.') -or $taskPart.IndexOfAny([IO.Path]::GetInvalidFileNameChars()) -ge 0) { throw 'Invalid payload destination.' } }
    }
    $taskPayloadCSharp = ($taskPayloadFiles | ForEach-Object { '"' + (Escape-CSharp $_) + '"' }) -join ', '
    $taskHeroRecord = Add-Input $taskHero $taskHeroSha
    $null = Add-Input $taskIcon
    $null = Add-Input $taskManifest
    $null = Add-Input $taskCompiler
    $null = Add-Input $PSCommandPath
    $null = Add-Input $taskConfiguration
    $taskSources = @(Get-ChildItem -LiteralPath $taskSource -File -Filter '*.cs' | Sort-Object Name)
    foreach ($taskFile in $taskSources) { $null = Add-Input $taskFile.FullName }
    foreach ($taskRequired in @('Program.cs', 'InstallerWindow.cs', 'InstallerService.cs', 'SetupContracts.cs')) {
        if ($taskRequired -notin $taskSources.Name) { throw "Missing installer source: $taskRequired" }
    }
    $taskReferences = @(
        (Join-Path $taskFramework 'WPF\WindowsBase.dll'),
        (Join-Path $taskFramework 'WPF\PresentationCore.dll'),
        (Join-Path $taskFramework 'WPF\PresentationFramework.dll'),
        (Join-Path $taskFramework 'System.Xaml.dll')
    )
    foreach ($taskReference in $taskReferences) { $null = Add-Input $taskReference }
    $taskConfig = Get-Content -LiteralPath $taskConfiguration -Raw | ConvertFrom-Json
    $taskVersion = [string]$taskConfig.version
    if ($taskVersion -notmatch '^\d+\.\d+\.\d+$') { throw 'The current setup builder requires a numeric three-part product version.' }
    $taskAssemblyVersion = "$taskVersion.0"
    $null = [IO.Directory]::CreateDirectory($taskOutput)
    $taskGenerated = Join-Path $taskOutput 'generated'
    $null = [IO.Directory]::CreateDirectory($taskGenerated)
    $taskBuildInfoPath = Join-Path $taskGenerated 'BuildInfo.cs'
    $taskBuildInfo = @"
namespace TokenMeow.Setup {
    internal static class BuildInfo {
        public const string ProductName = "Token 喵";
        public const string ProductVersion = "$(Escape-CSharp $taskVersion)";
        public const string EngineResourceName = "TokenMeow.Engine.exe";
        public const string EngineSha256 = "$($taskEngine.sha256)";
        public const string AppSha256 = "$($taskApplication.sha256)";
        public const string HeroResourceName = "TokenMeow.Hero.png";
        public static readonly string[] InstalledFiles = new string[] { $taskPayloadCSharp };
    }
}
"@
    Write-NewUtf8 $taskBuildInfoPath $taskBuildInfo
    $taskAssemblyInfoPath = Join-Path $taskGenerated 'AssemblyInfo.cs'
    $taskAssemblyInfo = @"
using System.Reflection;
using System.Runtime.Versioning;
[assembly: AssemblyTitle("Token 喵安装器")]
[assembly: AssemblyProduct("Token 喵")]
[assembly: AssemblyDescription("Token 喵的自绘安装界面")]
[assembly: AssemblyVersion("$taskAssemblyVersion")]
[assembly: AssemblyFileVersion("$taskAssemblyVersion")]
[assembly: TargetFramework(".NETFramework,Version=v4.8")]
"@
    Write-NewUtf8 $taskAssemblyInfoPath $taskAssemblyInfo
    $null = Add-Input $taskBuildInfoPath
    $null = Add-Input $taskAssemblyInfoPath
    $taskExeName = "TokenMeow_${taskVersion}_x64-custom-setup.exe"
    $taskExecutable = Join-Path $taskOutput $taskExeName
    $taskArguments = @('/nologo', '/target:winexe', '/platform:x64', '/optimize+', '/debug-', '/utf8output', '/codepage:65001')
    $taskArguments += '/out:"' + $taskExecutable + '"'
    $taskArguments += '/win32manifest:"' + $taskManifest + '"'
    $taskArguments += '/win32icon:"' + $taskIcon + '"'
    $taskArguments += '/resource:"' + $taskEngine.path + '",TokenMeow.Engine.exe'
    $taskArguments += '/resource:"' + $taskHero + '",TokenMeow.Hero.png'
    $taskArguments += @('/reference:System.dll', '/reference:System.Core.dll', '/reference:System.Security.dll', '/reference:System.Xml.dll')
    foreach ($taskReference in $taskReferences) { $taskArguments += '/reference:"' + $taskReference + '"' }
    foreach ($taskFile in $taskSources) { $taskArguments += '"' + $taskFile.FullName + '"' }
    $taskArguments += '"' + $taskBuildInfoPath + '"'
    $taskArguments += '"' + $taskAssemblyInfoPath + '"'
    $taskResponse = Join-Path $taskOutput 'compiler.rsp'
    Write-NewUtf8 $taskResponse ($taskArguments -join "`r`n")
    Write-NewUtf8 (Join-Path $taskOutput 'source-inputs.json') (($taskInputs.ToArray() | ConvertTo-Json -Depth 6) + "`n")
    $taskStarted = [DateTimeOffset]::UtcNow.ToString('O')
    & $taskCompiler "@$taskResponse" 1> (Join-Path $taskOutput 'compiler.stdout.log') 2> (Join-Path $taskOutput 'compiler.stderr.log')
    $taskExit = $LASTEXITCODE
    if ($taskExit -ne 0) { throw "Installer compiler exited $taskExit; preserve logs in $taskOutput" }
    $taskWindows = $taskConfig.bundle.windows
    if ($taskWindows.PSObject.Properties.Name -contains 'certificateThumbprint' -and $taskWindows.certificateThumbprint) {
        $taskThumbprint = [string]$taskWindows.certificateThumbprint
        if ($taskThumbprint -notmatch '^[A-Fa-f0-9]{40}$') { throw 'Invalid Windows code signing certificate thumbprint.' }
        $taskCertificate = Get-Item -LiteralPath ('Cert:\CurrentUser\My\' + $taskThumbprint) -ErrorAction SilentlyContinue
        if (-not $taskCertificate) { $taskCertificate = Get-Item -LiteralPath ('Cert:\LocalMachine\My\' + $taskThumbprint) -ErrorAction SilentlyContinue }
        if (-not $taskCertificate -or -not $taskCertificate.HasPrivateKey) { throw 'Windows code signing certificate/private key is unavailable.' }
        $taskSignArguments = @{ FilePath=$taskExecutable; Certificate=$taskCertificate; HashAlgorithm='SHA256' }
        if ($taskWindows.PSObject.Properties.Name -contains 'timestampUrl' -and $taskWindows.timestampUrl) { $taskSignArguments.TimestampServer=[string]$taskWindows.timestampUrl }
        $taskSigned = Set-AuthenticodeSignature @taskSignArguments
        if ($taskSigned.Status -ne 'Valid') { throw 'The outer setup signature is not valid; retain the failed build evidence.' }
    }
    foreach ($taskIndex in 0..($taskInputs.Count - 1)) {
        if ((Get-StreamSha $taskStreams[$taskIndex]) -cne $taskInputs[$taskIndex].sha256 -or $taskStreams[$taskIndex].Length -ne $taskInputs[$taskIndex].bytes) {
            throw 'Held source/resource input changed during compilation.'
        }
    }
    $taskReceipt = [ordered]@{
        status = 'CUSTOM_INSTALLER_COMPILED_NOT_INSTALLED'
        startedAtUtc = $taskStarted
        completedAtUtc = [DateTimeOffset]::UtcNow.ToString('O')
        productVersion = $taskVersion
        executable = [ordered]@{ path = $taskExecutable; bytes = (Get-Item -LiteralPath $taskExecutable).Length; sha256 = (Get-FileHash -LiteralPath $taskExecutable -Algorithm SHA256).Hash }
        engine = $taskEngine
        application = $taskApplication
        hero = $taskHeroRecord
        embeddedResources = @('TokenMeow.Engine.exe', 'TokenMeow.Hero.png')
        compilerExitCode = $taskExit
        sourceInputCount = $taskInputs.Count
        heldInputsPostHashMatched = $true
        frameworkTarget = '.NET Framework 4.8'
        uiRequiresWebView2 = $false
        installationExecuted = $false
        upgradeQualified = $false
        uninstallQualified = $false
        wholeV1Qualified = $false
    }
    foreach ($taskStream in $taskStreams) { $taskStream.Dispose() }
    $taskClosed = $true
    $taskReceipt['allInputHandlesClosed'] = $true
    Write-NewUtf8 (Join-Path $taskOutput 'build-receipt.json') (($taskReceipt | ConvertTo-Json -Depth 8) + "`n")
    $taskReceipt | ConvertTo-Json -Depth 8
} finally {
    if (-not $taskClosed) { foreach ($taskStream in $taskStreams) { $taskStream.Dispose() } }
}
