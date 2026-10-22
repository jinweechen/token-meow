#requires -Version 7.0
param()
$ErrorActionPreference='Stop'
$taskRoot=Split-Path -Parent $PSScriptRoot
$taskOutput=Join-Path $taskRoot ('output/uninstall-policy-'+(Get-Date -Format 'yyyyMMdd-HHmmss'))
if(Test-Path -LiteralPath $taskOutput){throw 'Use a new test output directory.'}
New-Item -ItemType Directory -Path $taskOutput|Out-Null
$taskInclude=Join-Path $taskRoot 'src-tauri/windows/installer-engine.nsh'
$taskSource=@'
Unicode true
RequestExecutionLevel user
SilentInstall silent
SilentUnInstall silent
!include LogicLib.nsh
!include FileFunc.nsh
!include "Win\RestartManager.nsh"
!include "__INCLUDE__"
Name "TokenMeow uninstall policy isolated fixture"
OutFile "__OUTPUT__\driver.exe"
Var DataPath
Section
  WriteUninstaller "$EXEDIR\uninstall.exe"
SectionEnd
Section Uninstall
  ReadEnvStr $DataPath "TOKEN_MEOW_UNINSTALL_FIXTURE"
  FileOpen $0 "$EXEDIR\last-data-path.txt" w
  FileWrite $0 "$DataPath"
  FileClose $0
  StrCpy $TM_UninstallError 0
  Push $DataPath
  Call un.TM_DeletePlainDataTree
  ${If} $TM_UninstallError != 0
    SetErrorLevel 71
  ${EndIf}
SectionEnd
'@.Replace('__INCLUDE__',$taskInclude).Replace('__OUTPUT__',$taskOutput)
$taskNsi=Join-Path $taskOutput 'policy.nsi'
$taskSource|Set-Content -LiteralPath $taskNsi -Encoding utf8BOM
$taskCompiler=Join-Path $env:LOCALAPPDATA 'tauri/NSIS/makensis.exe'
& $taskCompiler /V2 $taskNsi > (Join-Path $taskOutput 'compile.log') 2>&1
if($LASTEXITCODE -ne 0){Get-Content (Join-Path $taskOutput 'compile.log');throw 'NSIS policy compile failed.'}
& (Join-Path $taskOutput 'driver.exe') /S | Out-Null
if($LASTEXITCODE -ne 0){throw 'Isolated uninstaller creation failed.'}
$taskPlain=Join-Path $taskOutput 'plain data'
New-Item -ItemType Directory -Path (Join-Path $taskPlain 'child/empty') -Force|Out-Null
'fixture'|Set-Content -LiteralPath (Join-Path $taskPlain 'child/file.txt')
$env:TOKEN_MEOW_UNINSTALL_FIXTURE=$taskPlain
& (Join-Path $taskOutput 'uninstall.exe') /S ('_?='+$taskOutput) | Out-Null
if($LASTEXITCODE -ne 0 -or (Test-Path -LiteralPath $taskPlain)){throw "Plain data deletion failed, exit=$LASTEXITCODE; evidence=$taskOutput"}
$taskOutside=Join-Path $taskOutput 'protected-sentinel'
New-Item -ItemType Directory -Path $taskOutside|Out-Null
'must survive'|Set-Content -LiteralPath (Join-Path $taskOutside 'sentinel.txt')
$taskLinked=Join-Path $taskOutput 'linked-data'
New-Item -ItemType Directory -Path $taskLinked|Out-Null
New-Item -ItemType Junction -Path (Join-Path $taskLinked 'link') -Target $taskOutside|Out-Null
$env:TOKEN_MEOW_UNINSTALL_FIXTURE=$taskLinked
& (Join-Path $taskOutput 'uninstall.exe') /S ('_?='+$taskOutput) | Out-Null
if($LASTEXITCODE -ne 71 -or -not(Test-Path -LiteralPath (Join-Path $taskOutside 'sentinel.txt'))){throw 'Nested reparse point did not fail closed.'}
$env:TOKEN_MEOW_UNINSTALL_FIXTURE=Join-Path $taskLinked 'link'
& (Join-Path $taskOutput 'uninstall.exe') /S ('_?='+$taskOutput) | Out-Null
if($LASTEXITCODE -ne 71 -or -not(Test-Path -LiteralPath (Join-Path $taskOutside 'sentinel.txt'))){throw 'Root reparse point did not fail closed.'}
$taskLocked=Join-Path $taskOutput 'locked-data'
New-Item -ItemType Directory -Path $taskLocked|Out-Null
$taskLockedFile=Join-Path $taskLocked 'busy.txt'
'locked fixture'|Set-Content -LiteralPath $taskLockedFile
$taskHandle=[IO.File]::Open($taskLockedFile,'Open','Read','None')
try {
    $env:TOKEN_MEOW_UNINSTALL_FIXTURE=$taskLocked
    & (Join-Path $taskOutput 'uninstall.exe') /S ('_?='+$taskOutput) | Out-Null
    if($LASTEXITCODE -ne 71 -or -not(Test-Path -LiteralPath $taskLockedFile)){throw 'Locked file did not report incomplete cleanup.'}
} finally {$taskHandle.Dispose()}
[pscustomobject]@{status='passed';cases=4;output=$taskOutput;productionRegistryTouched=$false;productionInstallerExecuted=$false}|ConvertTo-Json|Tee-Object -FilePath (Join-Path $taskOutput 'result.json')
