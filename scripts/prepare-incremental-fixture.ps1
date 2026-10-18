# Creates only a fresh, content-free acceptance fixture. Never launches an app.
$ErrorActionPreference='Stop'
$meowWorkspace=[IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$meowOutputRoot=[IO.Path]::GetFullPath((Join-Path $meowWorkspace 'output'))
$meowCursor=$meowOutputRoot
while($meowCursor) {
    if(Test-Path -LiteralPath $meowCursor) {
        if((Get-Item -LiteralPath $meowCursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {throw 'Reparse points are not accepted for acceptance fixtures.'}
    }
    $meowParent=Split-Path -Parent $meowCursor
    if($meowParent -eq $meowCursor){break};$meowCursor=$meowParent
}
$meowFixtureRoot=Join-Path $meowOutputRoot ('performance\incremental-'+[Guid]::NewGuid().ToString('N'))
$meowSource=Join-Path $meowFixtureRoot 'source';$meowData=Join-Path $meowFixtureRoot 'data'
$meowLog=Join-Path $meowSource 'sessions\token-meow-p95.jsonl'
New-Item -ItemType Directory -Path (Split-Path -Parent $meowLog),$meowData -Force|Out-Null
$meowText=[IO.File]::ReadAllText((Join-Path $meowWorkspace 'tests\fixtures\codex-native.jsonl')).Replace('2026-10-15',[DateTime]::UtcNow.ToString('yyyy-MM-dd'))
if(-not $meowText.EndsWith("`n")){$meowText+="`n"}
[IO.File]::WriteAllText($meowLog,$meowText,[Text.UTF8Encoding]::new($false))
$meowManifest=[ordered]@{
    schemaVersion=1;fixtureKind='TokenMeow-visible-increment';id=[Guid]::NewGuid().ToString('D')
    sourceRoot=$meowSource;dataDirectory=$meowData;logPath=$meowLog
    threadId='00000000-0000-4000-8000-000000000001';baselineResponses=1;baselineTokens=1200
    eventCount=20;tokensPerEvent=1000;initialLogSha256=(Get-FileHash -LiteralPath $meowLog -Algorithm SHA256).Hash
    createdUtc=[DateTime]::UtcNow.ToString('o')
}
$meowManifestPath=Join-Path $meowFixtureRoot 'fixture.json'
[IO.File]::WriteAllText($meowManifestPath,($meowManifest|ConvertTo-Json -Depth 5),[Text.UTF8Encoding]::new($false))
[pscustomobject]@{fixtureManifest=$meowManifestPath;CODEX_HOME=$meowSource;TOKEN_MEOW_DATA_DIR=$meowData;applicationStarted=$false}|ConvertTo-Json
