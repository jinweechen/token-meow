#requires -Version 7.0
param([string]$OutputDirectory)
$ErrorActionPreference='Stop'
$taskRoot=Split-Path -Parent $PSScriptRoot
if(-not $OutputDirectory){$OutputDirectory=Join-Path $taskRoot ('output/installer-tests-'+(Get-Date -Format 'yyyyMMdd-HHmmss'))}
$taskOutput=[IO.Path]::GetFullPath($OutputDirectory)
if(-not $taskOutput.StartsWith((Join-Path $taskRoot 'output')+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Test output must be inside workspace output/.'}
if(Test-Path -LiteralPath $taskOutput){throw 'Preserve previous test evidence; use a new output directory.'}
New-Item -ItemType Directory -Path $taskOutput|Out-Null
$taskFramework=Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319'
$taskSource=Join-Path $taskRoot 'installer/TokenMeow.Setup'
@'
namespace TokenMeow.Setup { internal static class BuildInfo {
public const string ProductName="Token 喵", ProductVersion="0.1.1", EngineResourceName="unused", EngineSha256="unused", AppSha256="unused", HeroResourceName="unused";
public static readonly string[] InstalledFiles=new string[]{"token-meow.exe","licenses/manifest.json"};
} }
'@|Set-Content -LiteralPath (Join-Path $taskOutput 'BuildInfo.cs') -Encoding utf8
$taskReferences=@('System.dll','System.Core.dll','System.Security.dll','System.Xml.dll','System.Xaml.dll')+@('WindowsBase.dll','PresentationCore.dll','PresentationFramework.dll'|ForEach-Object{Join-Path $taskFramework ('WPF/'+$_)})
$taskArgs=@('/nologo','/platform:x64','/target:exe','/main:PolicyTests','/utf8output','/codepage:65001',('/out:'+(Join-Path $taskOutput 'PolicyTests.exe')))+@($taskReferences|ForEach-Object{'/reference:'+$_})+@(Get-ChildItem -LiteralPath $taskSource -Filter '*.cs' -File|Select-Object -ExpandProperty FullName)+@((Join-Path $taskSource 'tests/PolicyTests.cs'),(Join-Path $taskOutput 'BuildInfo.cs'))
& (Join-Path $taskFramework 'csc.exe') @taskArgs > (Join-Path $taskOutput 'compile.log') 2>&1
if($LASTEXITCODE -ne 0){Get-Content (Join-Path $taskOutput 'compile.log');throw 'Installer tests failed to compile.'}
& (Join-Path $taskOutput 'PolicyTests.exe') > (Join-Path $taskOutput 'tests.log') 2>&1
$taskExit=$LASTEXITCODE
Get-Content (Join-Path $taskOutput 'tests.log')
if($taskExit -ne 0){throw 'Installer tests failed.'}
[pscustomobject]@{status='passed';exitCode=$taskExit;output=$taskOutput;productionInstallExecuted=$false}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $taskOutput 'result.json')
