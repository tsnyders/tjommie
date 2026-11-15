# The readers behind collect-r1-facts.ps1 (T8.1). Dot-source this file; it defines functions only, and every function
# takes the place to look as a parameter, so a test can point it at a fixture. Nothing here installs, deletes or writes
# anything, and nothing here names a Tjommie location: the collecting script does that, and only on a hosted runner.
#
# Written for Windows PowerShell 5.1 and PowerShell 7 alike.

Set-StrictMode -Version Latest

# Test-PathInFolder and Get-CommandExecutable (pure functions, like everything in that file; it also defines the scoped stop).
. (Join-Path $PSScriptRoot 'scoped-processes.ps1')

$script:Views = @{ '64' = [Microsoft.Win32.RegistryView]::Registry64; '32' = [Microsoft.Win32.RegistryView]::Registry32 }

function Open-BaseKey([string]$Hive, [string]$View) {
    $hive = [Microsoft.Win32.RegistryHive]$Hive
    return [Microsoft.Win32.RegistryKey]::OpenBaseKey($hive, $script:Views[$View])
}

# One registry value as it is stored: its name, its type and its data, text as text, numbers as numbers, binary as hex.
# Environment variables in an expandable string are left unexpanded, as the installer wrote them.
function Get-RegistryValueFact($Key, [string]$Name) {
    $kind = $Key.GetValueKind($Name)
    $data = $Key.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    if ($kind -eq [Microsoft.Win32.RegistryValueKind]::Binary) {
        $data = (@($data | ForEach-Object { '{0:x2}' -f $_ }) -join ' ')
    } elseif ($kind -eq [Microsoft.Win32.RegistryValueKind]::MultiString) {
        $data = @($data)
    }
    return [ordered]@{ name = $Name; kind = [string]$kind; data = $data }
}

# A key and everything in it (values and the names of its subkeys), or $null if the key is not there.
function Get-RegistryKeyFact([string]$Hive, [string]$SubKey, [string]$View = '64') {
    $base = Open-BaseKey $Hive $View
    try {
        $key = $base.OpenSubKey($SubKey)
        if ($null -eq $key) { return $null }
        try {
            $values = @()
            foreach ($name in ($key.GetValueNames() | Sort-Object)) { $values += , (Get-RegistryValueFact $key $name) }
            return [ordered]@{
                hive    = $Hive
                key     = $SubKey
                view    = $View
                values  = $values
                subKeys = @($key.GetSubKeyNames() | Sort-Object)
            }
        } finally { $key.Close() }
    } finally { $base.Close() }
}

# The text of a registry value, or $null if it is not there (or not text).
function Get-RegistryText($Key, [string]$Name) {
    $value = $Key.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    if ($null -eq $value -or $value -isnot [string] -or $value -eq '') { return $null }
    return $value
}

function Test-SameFolder([string]$A, [string]$B) {
    try { return [IO.Path]::GetFullPath($A).TrimEnd('\') -ieq [IO.Path]::GetFullPath($B).TrimEnd('\') } catch { return $false }
}

# Whether an uninstall entry is this install's, which is whether it points into $Folder. By its InstallLocation if it has one
# (case and a trailing slash don't matter); electron-builder's per-user installs have none there (the location is kept under
# HKCU\Software\<GUID>), so then by the program its uninstall command, quiet uninstall command or icon names. An entry whose
# location says another folder is not this install's, whatever else it points at.
function Test-EntryPointsIntoFolder($Entry, [string]$Folder) {
    $location = Get-RegistryText $Entry 'InstallLocation'
    if ($location) { return (Test-SameFolder $location $Folder) }
    foreach ($name in 'QuietUninstallString', 'UninstallString', 'DisplayIcon') {
        $text = Get-RegistryText $Entry $name
        if (-not $text) { continue }
        # An icon is "path" or "path,index"; the others are command lines.
        $program = if ($name -eq 'DisplayIcon') { ($text.Trim() -replace ',\s*-?\d+$', '').Trim('"') } else { Get-CommandExecutable $text }
        if ($program -and (Test-PathInFolder $program $Folder)) { return $true }
    }
    return $false
}

# The names of the uninstall entries under $UninstallKey (in $Hive) that are the install in $InstallFolder: exactly the ones
# that point into it, so another version, another app and an entry for another folder never count.
function Find-UninstallEntries([string]$Hive, [string]$UninstallKey, [string]$InstallFolder, [string]$View = '64') {
    $base = Open-BaseKey $Hive $View
    try {
        $parent = $base.OpenSubKey($UninstallKey)
        if ($null -eq $parent) { return @() }
        try {
            $found = @()
            foreach ($name in ($parent.GetSubKeyNames() | Sort-Object)) {
                $entry = $parent.OpenSubKey($name)
                try { if (Test-EntryPointsIntoFolder $entry $InstallFolder) { $found += $name } } finally { $entry.Close() }
            }
            return $found
        } finally { $parent.Close() }
    } finally { $base.Close() }
}

# Every uninstall entry under $UninstallKey, matched or not, with the facts that show what it is: its name, DisplayName,
# InstallLocation, UninstallString, QuietUninstallString, DisplayIcon and Publisher ($null where it has none). Taken before
# anything is asserted, so a run that cannot find the app's entry still shows what is there.
function Get-UninstallSurvey([string]$Hive, [string]$UninstallKey, [string]$View = '64') {
    $base = Open-BaseKey $Hive $View
    try {
        $parent = $base.OpenSubKey($UninstallKey)
        if ($null -eq $parent) { return @() }
        try {
            $entries = @()
            foreach ($name in ($parent.GetSubKeyNames() | Sort-Object)) {
                $entry = $parent.OpenSubKey($name)
                try {
                    $entries += , ([ordered]@{
                        hive                 = $Hive
                        view                 = $View
                        subKey               = $name
                        displayName          = Get-RegistryText $entry 'DisplayName'
                        installLocation      = Get-RegistryText $entry 'InstallLocation'
                        uninstallString      = Get-RegistryText $entry 'UninstallString'
                        quietUninstallString = Get-RegistryText $entry 'QuietUninstallString'
                        displayIcon          = Get-RegistryText $entry 'DisplayIcon'
                        publisher            = Get-RegistryText $entry 'Publisher'
                    })
                } finally { $entry.Close() }
            }
            return $entries
        } finally { $parent.Close() }
    } finally { $base.Close() }
}

# The subkeys of $ParentKey that have an InstallLocation value: where an install records itself (electron-builder keeps its
# location in HKCU\Software\<GUID>), found without knowing the GUID.
function Get-InstallLocationKeys([string]$Hive, [string]$ParentKey, [string]$View = '64') {
    $base = Open-BaseKey $Hive $View
    try {
        $parent = $base.OpenSubKey($ParentKey)
        if ($null -eq $parent) { return @() }
        try {
            $found = @()
            foreach ($name in ($parent.GetSubKeyNames() | Sort-Object)) {
                $child = $parent.OpenSubKey($name)
                try {
                    $location = Get-RegistryText $child 'InstallLocation'
                    if ($location) { $found += , ([ordered]@{ hive = $Hive; view = $View; key = "$ParentKey\$name"; installLocation = $location }) }
                } finally { $child.Close() }
            }
            return $found
        } finally { $parent.Close() }
    } finally { $base.Close() }
}
# Every file under $Folder (sorted), each with its path relative to the folder. The relative path is taken off a root that is spelled as
# the file system spells it: Resolve-Path keeps an 8.3 alias as it was given (a profile folder such as RUNNER~1), where Get-Item and Get-ChildItem
# return the long spelling (runneradmin), so a prefix cut off by the length of a root as given left debris on the front ("ta\new.txt")
# on a GitHub-hosted runner, whose profile and temp folders are spelled that way. Here the root and the files come from the same
# listing, and a file that is not under that root is an error, never a wrong path.
# Returns objects with File (the FileInfo) and Relative; nothing if the folder holds no file. Throws if the folder is not there.
function Get-FilesUnder([string]$Folder, [string]$Filter = '*') {
    $base = (Get-Item -LiteralPath $Folder).FullName.TrimEnd('\')
    foreach ($file in @(Get-ChildItem -LiteralPath $base -Recurse -File -Force -Filter $Filter | Sort-Object FullName)) {
        if (-not $file.FullName.StartsWith($base + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "$($file.FullName) is not under $base." }
        [pscustomobject]@{ File = $file; Relative = $file.FullName.Substring($base.Length + 1) }
    }
}

# Every file under $Dir, with sizes: the install's file inventory. Paths are relative, with backslashes, as Windows shows
# them; the uninstaller NSIS adds is counted apart, because a fresh build's folder has none.
function Get-FileInventory([string]$Dir, [string]$UninstallerPattern = 'Uninstall *.exe') {
    $files = @(Get-FilesUnder $Dir)
    $list = @()
    $uninstallers = @()
    $bytes = 0L
    foreach ($item in $files) {
        $file = $item.File
        $relative = $item.Relative
        $list += , ([ordered]@{ path = $relative; bytes = $file.Length })
        $bytes += $file.Length
        if ($relative -notlike '*\*' -and $file.Name -like $UninstallerPattern) { $uninstallers += $relative }
    }
    return [ordered]@{
        count                   = $files.Count
        countWithoutUninstaller = $files.Count - $uninstallers.Count
        uninstallers            = @($uninstallers)
        totalBytes              = $bytes
        files                   = $list
    }
}

# The version resource of an executable: the strings Windows shows on its Details tab.
function Get-ExeVersionFact([string]$Path) {
    $info = (Get-Item -LiteralPath $Path).VersionInfo
    return [ordered]@{
        companyName      = $info.CompanyName
        fileDescription  = $info.FileDescription
        fileVersion      = $info.FileVersion
        productName      = $info.ProductName
        productVersion   = $info.ProductVersion
        originalFilename = $info.OriginalFilename
        internalName     = $info.InternalName
        legalCopyright   = $info.LegalCopyright
    }
}

# The names, and whether each is a file or a folder, directly inside $Dir: what a first start leaves there (never their
# contents). $null if $Dir is not there.
function Get-FolderEntries([string]$Dir) {
    if (-not (Test-Path -LiteralPath $Dir -PathType Container)) { return $null }
    $entries = @()
    foreach ($item in (Get-ChildItem -LiteralPath $Dir -Force | Sort-Object Name)) {
        $entries += , ([ordered]@{ name = $item.Name; type = $(if ($item.PSIsContainer) { 'folder' } else { 'file' }); bytes = $(if ($item.PSIsContainer) { $null } else { $item.Length }) })
    }
    return $entries
}

# Every shortcut (*.lnk) under $Folder: where it points and what Windows keys its notifications and taskbar button by,
# its AppUserModelID. The ID is a property of the .lnk file, read through the shell as the Details pane reads it.
function Get-ShortcutFacts([string]$Folder) {
    if (-not (Test-Path -LiteralPath $Folder -PathType Container)) { return @() }
    $wsh = New-Object -ComObject WScript.Shell
    $shell = New-Object -ComObject Shell.Application
    $found = @()
    foreach ($entry in @(Get-FilesUnder $Folder '*.lnk')) {
        $file = $entry.File
        $link = $wsh.CreateShortcut($file.FullName)
        $item = $shell.NameSpace($file.DirectoryName).ParseName($file.Name)
        $aumid = $null
        $activator = $null
        if ($null -ne $item) {
            $aumid = $item.ExtendedProperty('System.AppUserModel.ID')
            $activator = $item.ExtendedProperty('System.AppUserModel.ToastActivatorCLSID')
        }
        $found += , ([ordered]@{
            path                 = $entry.Relative
            target               = $link.TargetPath
            arguments            = $link.Arguments
            workingDirectory     = $link.WorkingDirectory
            iconLocation         = $link.IconLocation
            description          = $link.Description
            windowStyle          = $link.WindowStyle
            hotkey               = $link.Hotkey
            appUserModelId       = $(if ($null -eq $aumid) { '' } else { [string]$aumid })
            toastActivatorClsid  = $(if ($null -eq $activator) { '' } else { [string]$activator })
        })
    }
    return $found
}

# The environment folders the paths in a facts file start with, so a reader can turn a path on the runner into
# %LOCALAPPDATA%\... as an installer script writes it.
function Get-PathTokens {
    $tokens = [ordered]@{}
    foreach ($name in 'LOCALAPPDATA', 'APPDATA', 'USERPROFILE', 'TEMP') {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ($value) { $tokens["%$name%"] = $value }
    }
    return $tokens
}

# Windows' own folders for the shortcuts: the user's Start menu and desktop, and the all-users ones (which an install for
# one user must not use).
function Get-ShortcutFolders {
    return [ordered]@{
        userStartMenu     = [Environment]::GetFolderPath('Programs')
        userDesktop       = [Environment]::GetFolderPath('DesktopDirectory')
        allUsersStartMenu = [Environment]::GetFolderPath('CommonPrograms')
        allUsersDesktop   = [Environment]::GetFolderPath('CommonDesktopDirectory')
    }
}

# Processes that were not running before, with their command lines: what an installer starts while it runs, such as the
# old version's uninstaller and the arguments an upgrade gives it. $Before is the process ids at the start.
function Get-NewProcessFacts([int[]]$Before) {
    $found = @()
    foreach ($process in (Get-CimInstance Win32_Process)) {
        if ($Before -contains [int]$process.ProcessId) { continue }
        $found += , ([ordered]@{
            name           = $process.Name
            executablePath = $process.ExecutablePath
            commandLine    = $process.CommandLine
        })
    }
    return $found
}
