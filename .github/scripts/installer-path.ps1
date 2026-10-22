# Checks that a desktop app's Windows setup.exe puts its command line on
# the user's Path and that uninstalling takes it off again, for the
# release workflows.
#
#   installer-path.ps1 <product> <setup.exe> <version>
#
# The product, social-deduction or twenty-questions, is the command line's
# name, and in words, "Social Deduction", the install folder's.
#
# Gives the user's Path a %VARIABLE% entry and enough others to be longer
# than 1024 characters, then installs silently twice, as an upgrade does,
# and checks that the Path holds the install folder once, as its type
# REG_EXPAND_SZ, and that a new shell's <product> --version prints
# <version>. Uninstalls and checks that the Path is exactly as it was.
# Puts the user's own Path back at the end.
param(
    [Parameter(Mandatory)] [ValidateSet('social-deduction', 'twenty-questions')] [string] $Product,
    [Parameter(Mandatory)] [string] $Setup,
    [Parameter(Mandatory)] [string] $Version
)
$ErrorActionPreference = 'Stop'

$title = (Get-Culture).TextInfo.ToTitleCase($Product.Replace('-', ' '))
$folder = Join-Path $env:LOCALAPPDATA $title
$key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
$failures = 0

function Get-UserPath {
    $key.GetValue('Path', $null, 'DoNotExpandEnvironmentNames')
}

function Check([string] $what, [bool] $ok) {
    if ($ok) { "ok: $what" } else { Write-Host "FAIL: $what"; $script:failures++ }
}

function Wait-Run([string] $exe, [string] $arguments) {
    $process = Start-Process -FilePath $exe -ArgumentList $arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "$exe exited with $($process.ExitCode)" }
}

$own = Get-UserPath
$ownKind = if ($null -ne $own) { $key.GetValueKind('Path') } else { $null }
try {
    $long = @($own) + '%USERPROFILE%\installer-path-check' +
        (1..40 | ForEach-Object { "C:\installer-path-check\a-folder-of-some-length-$_" })
    $before = ($long | Where-Object { $_ }) -join ';'
    $key.SetValue('Path', $before, 'ExpandString')
    Check "the Path starts longer than 1024 characters" ($before.Length -gt 1024)

    Wait-Run $Setup '/S'
    Wait-Run $Setup '/S'
    $after = Get-UserPath
    $entries = @($after -split ';' | Where-Object { $_.TrimEnd('\') -eq $folder })
    Check "the Path holds the install folder once" ($entries.Count -eq 1)
    Check "the rest of the Path is as it was" ($after -eq "$before;$folder")
    Check "the Path is still REG_EXPAND_SZ" ($key.GetValueKind('Path') -eq 'ExpandString')

    # A new shell, with the environment a newly opened terminal gets.
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $user = [Environment]::ExpandEnvironmentVariables($after)
    $env:Path = "$machine;$user"
    $found = (Get-Command $Product).Source
    Check "a new shell finds the installed $Product" ($found -eq (Join-Path $folder "$Product.exe"))
    $printed = powershell -NoProfile -Command "$Product --version"
    "$Product --version: $printed"
    Check "$Product --version prints $Version" ($printed -match [regex]::Escape($Version))

    # Run in place (_?=, last and unquoted), so that waiting for it waits
    # for the uninstall.
    Wait-Run (Join-Path $folder 'uninstall.exe') "/S _?=$folder"
    Remove-Item -Recurse -Force $folder
    Check "uninstalling leaves the Path exactly as it was" ((Get-UserPath) -eq $before)
} finally {
    if ($null -ne $own) { $key.SetValue('Path', $own, $ownKind) } else { $key.DeleteValue('Path', $false) }
    $key.Close()
}

if ($failures -gt 0) { throw "$failures check(s) failed" }
