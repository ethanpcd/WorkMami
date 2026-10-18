# probe-winbuddy.ps1
# WorkMami - Windows WorkBuddy layout probe (ASCII only to avoid encoding issues).
#
# Usage (on the Windows machine that has WorkBuddy installed):
#   powershell -ExecutionPolicy Bypass -File .\probe-winbuddy.ps1
#
# It writes probe-winbuddy.json to the current folder. Send that file back
# so the Windows paths / encryption locations can be adapted.
# This script only reads metadata (path / size / time / existence). It never
# reads or prints any token plaintext.

$ErrorActionPreference = "SilentlyContinue"

function Test-Path-Info($path) {
    if (-not $path) { return $null }
    if (Test-Path -LiteralPath $path) {
        $item = Get-Item -LiteralPath $path
        $size = $null
        if (-not $item.PSIsContainer) { $size = $item.Length }
        return [ordered]@{
            exists   = $true
            isDir    = $item.PSIsContainer
            size     = $size
            modified = $item.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss")
        }
    }
    return [ordered]@{ exists = $false }
}

function List-Children($path) {
    $result = @()
    if (-not (Test-Path -LiteralPath $path)) { return $result }
    Get-ChildItem -LiteralPath $path -Force | ForEach-Object {
        $size = $null
        if (-not $_.PSIsContainer) { $size = $_.Length }
        $result += [ordered]@{
            name     = $_.Name
            isDir    = $_.PSIsContainer
            size     = $size
            modified = $_.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss")
        }
    }
    return $result
}

$appData      = $env:APPDATA
$localAppData = $env:LOCALAPPDATA
$userProfile  = $env:USERPROFILE

# auth dir (WorkBuddy / CodeBuddy common layout)
$authDir        = Join-Path $appData "CodeBuddyExtension\Data\Public\auth"
$desktopInfo    = Join-Path $authDir "workbuddy-desktop.info"
$desktopInfoDev = Join-Path $authDir "workbuddy-desktop-dev.info"

# ~/.workbuddy
$wbHome        = Join-Path $userProfile ".workbuddy"
$keyblob       = Join-Path $wbHome "keyblob"
$failover      = Join-Path $wbHome "failover.json"
$wbDb          = Join-Path $wbHome "workbuddy.db"
$sessionsDir   = Join-Path $wbHome "sessions"
$memoryDir     = Join-Path $wbHome "memory"
$skillsDir     = Join-Path $wbHome "skills"
$rulesDir      = Join-Path $wbHome "rules"
$securityDir   = Join-Path $wbHome "security"
$keyFallback   = Join-Path $userProfile ".workbuddy-key-fallback"
$masterKeyFb   = Join-Path $keyFallback ".master.key"
$workspace     = Join-Path $userProfile "WorkBuddy"

$authChildren = List-Children $authDir

# user-*-personal dirs
$personalDirs = @()
if (Test-Path $wbHome) {
    Get-ChildItem -LiteralPath $wbHome -Directory -Force |
        Where-Object { $_.Name -like "user-*-personal" } |
        ForEach-Object { $personalDirs += $_.Name }
}

# security/<uid> dirs (historical login hints)
$securityUids = @()
if (Test-Path $securityDir) {
    Get-ChildItem -LiteralPath $securityDir -Directory -Force |
        ForEach-Object { $securityUids += $_.Name }
}

# Windows Credential Manager entries related to WorkBuddy/CodeBuddy (names only)
$credTargets = @()
$cmdkeyOut = & cmdkey /list 2>$null
if ($cmdkeyOut) {
    foreach ($line in $cmdkeyOut) {
        if ($line -match "(?i)workbuddy|codebuddy|workmami|safe storage") {
            $credTargets += $line.Trim()
        }
    }
}

# Registry uninstall info to locate install path
$registry = @()
$uninstallRoots = @(
    "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall",
    "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall",
    "HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"
)
foreach ($root in $uninstallRoots) {
    Get-ChildItem -LiteralPath $root -Force | ForEach-Object {
        $p = Get-ItemProperty -LiteralPath $_.PSPath
        if ($p.DisplayName -match "(?i)WorkBuddy|CodeBuddy") {
            $registry += [ordered]@{
                name = $p.DisplayName
                path = $p.InstallLocation
                exe  = $p.DisplayIcon
                root = $root
            }
        }
    }
}

$report = [ordered]@{
    generatedAt = (Get-Date).ToString("yyyy-MM-dd HH:mm:ss")
    platform    = "windows"
    env = [ordered]@{
        APPDATA      = $appData
        LOCALAPPDATA = $localAppData
        USERPROFILE  = $userProfile
        computerName = $env:COMPUTERNAME
    }
    auth = [ordered]@{
        dir            = Test-Path-Info $authDir
        desktopInfo    = Test-Path-Info $desktopInfo
        desktopInfoDev = Test-Path-Info $desktopInfoDev
        children       = $authChildren
    }
    workbuddy = [ordered]@{
        home         = Test-Path-Info $wbHome
        keyblob      = Test-Path-Info $keyblob
        failover     = Test-Path-Info $failover
        db           = Test-Path-Info $wbDb
        sessionsDir  = Test-Path-Info $sessionsDir
        memoryDir    = Test-Path-Info $memoryDir
        skillsDir    = Test-Path-Info $skillsDir
        rulesDir     = Test-Path-Info $rulesDir
        securityDir  = Test-Path-Info $securityDir
        personalDirs = $personalDirs
        securityUids = $securityUids
        workspace    = Test-Path-Info $workspace
    }
    keyFallback = [ordered]@{
        dir       = Test-Path-Info $keyFallback
        masterKey = Test-Path-Info $masterKeyFb
    }
    credentials = $credTargets
    registry    = $registry
}

$outFile = Join-Path (Get-Location) "probe-winbuddy.json"
$report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $outFile -Encoding UTF8

Write-Host "Probe finished. Result written to: $outFile" -ForegroundColor Green
Write-Host "Please send this JSON file back for Windows adaptation." -ForegroundColor Cyan
Write-Host ""
Write-Host "=== Summary ===" -ForegroundColor Yellow
Write-Host ("desktop.info exists : " + (Test-Path $desktopInfo))
Write-Host (".workbuddy exists   : " + (Test-Path $wbHome))
Write-Host ("workbuddy.db exists : " + (Test-Path $wbDb))
Write-Host ("personal dirs       : " + $personalDirs.Count)
Write-Host ("security accounts   : " + $securityUids.Count)
Write-Host ("credential entries  : " + $credTargets.Count)
