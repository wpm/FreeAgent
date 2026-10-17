# Adds a folder to the user's Path, or removes it, for the Windows
# installer's hooks in installer-hooks.nsh.
#
#   user-path.ps1 -Add <folder>
#   user-path.ps1 -Remove <folder>
#
# Works on the Path value under HKCU\Environment as it is stored, with its
# %VARIABLES% unexpanded, and writes it back with the type it had
# (REG_EXPAND_SZ for a new one), so a Path of any length comes back whole.
# Entries match ignoring case and a trailing backslash. Adding a folder
# already there changes nothing, and removing one takes out its entries
# and leaves every other character as it was. The installer tells running
# programs about the change.
param(
    [Parameter(Mandatory, ParameterSetName = 'Add')] [string] $Add,
    [Parameter(Mandatory, ParameterSetName = 'Remove')] [string] $Remove
)
$ErrorActionPreference = 'Stop'

function Test-Folder([string] $entry, [string] $folder) {
    $entry.TrimEnd('\') -eq $folder.TrimEnd('\')
}

$key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
try {
    $path = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
    $kind = if ($null -ne $key.GetValue('Path')) { $key.GetValueKind('Path') } else { 'ExpandString' }
    $entries = $path -split ';'

    if ($Add) {
        if (-not ($entries | Where-Object { Test-Folder $_ $Add })) {
            $separator = if ($path -eq '' -or $path.EndsWith(';')) { '' } else { ';' }
            $key.SetValue('Path', $path + $separator + $Add, $kind)
        }
    } else {
        $rest = @($entries | Where-Object { -not (Test-Folder $_ $Remove) })
        if ($rest.Count -lt $entries.Count) {
            $left = $rest -join ';'
            if ($left -ne '') { $key.SetValue('Path', $left, $kind) } else { $key.DeleteValue('Path') }
        }
    }
} finally {
    $key.Close()
}
