# The readers behind collect-r1-facts.ps1 (T8.1). Dot-source this file; it defines functions only, and every function
# takes the place to look as a parameter, so a test can point it at a fixture. Nothing here installs, deletes or writes
# anything, and nothing here names a Tjommie location: the collecting script does that, and only on a hosted runner.
#
# Written for Windows PowerShell 5.1 and PowerShell 7 alike.

Set-StrictMode -Version Latest

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

# The uninstall entries under $UninstallKey (in $Hive) that belong to the app: DisplayName is the product name, or
# InstallLocation is the install folder.
function Find-UninstallEntries([string]$Hive, [string]$UninstallKey, [string]$ProductName, [string]$InstallFolder, [string]$View = '64') {
    $base = Open-BaseKey $Hive $View
    try {
        $parent = $base.OpenSubKey($UninstallKey)
        if ($null -eq $parent) { return @() }
        try {
            $found = @()
            foreach ($name in ($parent.GetSubKeyNames() | Sort-Object)) {
                $entry = $parent.OpenSubKey($name)
                try {
                    $display = $entry.GetValue('DisplayName')
                    $location = $entry.GetValue('InstallLocation')
                    $same = $null -ne $location -and ([string]$location).TrimEnd('\') -ieq $InstallFolder.TrimEnd('\')
                    if ($display -eq $ProductName -or $same) { $found += $name }
                } finally { $entry.Close() }
            }
            return $found
        } finally { $parent.Close() }
    } finally { $base.Close() }
}

# Every file under $Dir, with sizes: the install's file inventory. Paths are relative, with backslashes, as Windows shows
# them; the uninstaller NSIS adds is counted apart, because a fresh build's folder has none.
function Get-FileInventory([string]$Dir, [string]$UninstallerPattern = 'Uninstall *.exe') {
    $root = (Resolve-Path -LiteralPath $Dir).Path.TrimEnd('\')
    $files = @(Get-ChildItem -LiteralPath $root -Recurse -File -Force | Sort-Object FullName)
    $list = @()
    $uninstallers = @()
    $bytes = 0L
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($root.Length + 1)
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
    $root = (Resolve-Path -LiteralPath $Folder).Path.TrimEnd('\')
    $wsh = New-Object -ComObject WScript.Shell
    $shell = New-Object -ComObject Shell.Application
    $found = @()
    foreach ($file in (Get-ChildItem -LiteralPath $root -Recurse -File -Filter '*.lnk' -Force | Sort-Object FullName)) {
        $link = $wsh.CreateShortcut($file.FullName)
        $item = $shell.NameSpace($file.DirectoryName).ParseName($file.Name)
        $aumid = $null
        $activator = $null
        if ($null -ne $item) {
            $aumid = $item.ExtendedProperty('System.AppUserModel.ID')
            $activator = $item.ExtendedProperty('System.AppUserModel.ToastActivatorCLSID')
        }
        $found += , ([ordered]@{
            path                 = $file.FullName.Substring($root.Length + 1)
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
