# Per-user setup for the portable Windows Token Ashtray archive.
param(
    [ValidateSet('Install', 'Uninstall')][string]$Action = 'Install',
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\Token Ashtray'),
    [string]$PreviousDir,
    [switch]$RemoveSettings,
    # The graphical installer uses the same read-only ownership/running checks
    # before its rollback-capable file transaction starts.
    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
$markerName = '.token-ashtray-install'
$markerText = "Token Ashtray per-user installation`n"
$shortcutDescription = 'Token Ashtray per-user launcher'
$runPath = 'Software\Microsoft\Windows\CurrentVersion\Run'
$runName = 'Token Ashtray'
$startupApprovedPath = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'

function FullPath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { throw 'Installation path is empty.' }
    return [IO.Path]::GetFullPath($Path)
}

function SamePath([string]$Left, [string]$Right) {
    return [string]::Equals((FullPath $Left), (FullPath $Right), [StringComparison]::OrdinalIgnoreCase)
}

function CheckRegularFile([string]$Path) {
    if (Test-Path -LiteralPath $Path) {
        $item = Get-Item -LiteralPath $Path -Force
        if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Expected an ordinary file: $Path"
        }
    }
}

function Installed([string]$Directory, [bool]$AllowRemoved = $false) {
    if (Test-Path -LiteralPath $Directory) {
        $item = Get-Item -LiteralPath $Directory -Force
        if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Expected an ordinary installation directory: $Directory"
        }
    } else { return $false }
    $marker = Join-Path $Directory $markerName
    CheckRegularFile $marker
    if (Test-Path -LiteralPath $marker) {
        if ([IO.File]::ReadAllText($marker) -ne $markerText) {
            throw "Installation marker is unrelated: $marker"
        }
        return $true
    }
    if (@(Get-ChildItem -LiteralPath $Directory -Force).Count -gt 0) {
        if ($AllowRemoved) {
            $binary = Join-Path $Directory 'token-ashtray.exe'
            CheckRegularFile $binary
            if ([IO.File]::Exists($binary)) { throw "Unowned app-named file in installation directory: $binary" }
            return $false
        }
        throw "Installation directory contains unrelated files: $Directory"
    }
    return $false
}

function CommandFor([string]$Directory) {
    return '"' + (Join-Path $Directory 'token-ashtray.exe') + '" --autostart'
}

function ReadShortcut([string]$Path, [string[]]$AcceptedTargets) {
    CheckRegularFile $Path
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($Path)
    $targetAccepted = $false
    foreach ($target in $AcceptedTargets) {
        if (SamePath $shortcut.TargetPath $target) { $targetAccepted = $true }
    }
    if ($shortcut.Description -ne $shortcutDescription -or -not $targetAccepted -or $shortcut.Arguments) {
        throw "Unrelated or stale Start menu shortcut at $Path; resolve it before setup."
    }
    return $true
}

function CopyAtomic([string]$Source, [string]$Destination) {
    CheckRegularFile $Destination
    $parent = [IO.Path]::GetDirectoryName($Destination)
    $temporary = Join-Path $parent ([IO.Path]::GetRandomFileName())
    $backup = Join-Path $parent ([IO.Path]::GetRandomFileName())
    try {
        [IO.File]::Copy($Source, $temporary)
        if ([IO.File]::Exists($Destination)) {
            [IO.File]::Replace($temporary, $Destination, $backup)
        } else {
            [IO.File]::Move($temporary, $Destination)
        }
    } finally {
        if ([IO.File]::Exists($temporary)) { [IO.File]::Delete($temporary) }
        if ([IO.File]::Exists($backup)) { [IO.File]::Delete($backup) }
    }
}

function RemoveOwnedFiles([string]$Directory) {
    if (-not (Installed $Directory $true)) { return }
    foreach ($name in @('token-ashtray.exe', $markerName)) {
        $path = Join-Path $Directory $name
        CheckRegularFile $path
        if ([IO.File]::Exists($path)) { [IO.File]::Delete($path) }
    }
    if (@(Get-ChildItem -LiteralPath $Directory -Force).Count -eq 0) {
        [IO.Directory]::Delete($Directory)
    } else {
        Write-Warning "Preserved other files in $Directory"
    }
}

# Windows Settings and Task Manager record a Start at login switch under the
# Run value's name. Removing it is safe: an absent switch means enabled.
function RemoveStartupSwitch([string]$Path, [string]$Name) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Path, $true)
    if ($null -eq $key) { return }
    try { $key.DeleteValue($Name, $false) } finally { $key.Close() }
}

function RunningFrom([string[]]$Directories) {
    $running = @()
    foreach ($process in @(Get-Process -Name 'token-ashtray' -ErrorAction SilentlyContinue)) {
        if (-not $process.Path) { continue }
        $folder = [IO.Path]::GetDirectoryName($process.Path)
        foreach ($directory in $Directories) {
            if (SamePath $folder $directory) { $running += $process.Id; break }
        }
    }
    return $running
}

function RemoveSettingsFile([string]$Settings) {
    CheckRegularFile $Settings
    if ([IO.File]::Exists($Settings)) { [IO.File]::Delete($Settings) }
    $settingsDir = [IO.Path]::GetDirectoryName($Settings)
    if ([IO.Directory]::Exists($settingsDir) -and
        @(Get-ChildItem -LiteralPath $settingsDir -Force).Count -eq 0) {
        [IO.Directory]::Delete($settingsDir)
    }
}

try {
    if ($Action -eq 'Install' -and $RemoveSettings) { throw '-RemoveSettings is only valid with Uninstall.' }
    if ($Action -eq 'Uninstall' -and $PreviousDir) { throw '-PreviousDir is only valid with Install.' }
    $destination = FullPath $InstallDir
    $previous = if ($PreviousDir) { FullPath $PreviousDir } else { $destination }
    if (-not (SamePath $previous $destination) -and -not (Installed $previous)) {
        throw "Previous installation is missing or unowned: $previous"
    }
    $null = Installed $destination ($Action -eq 'Uninstall')
    # A running executable cannot be replaced or deleted; stop before changing anything.
    $running = @(RunningFrom @($destination, $previous))
    if ($running.Count -gt 0) {
        throw "Token Ashtray is running (process $($running -join ', ')). Quit it from its tray menu, then run setup again. Nothing was changed."
    }
    $shortcutPath = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Token Ashtray.lnk'
    $currentExe = Join-Path $destination 'token-ashtray.exe'
    $previousExe = Join-Path $previous 'token-ashtray.exe'
    $shortcutExists = ReadShortcut $shortcutPath @($currentExe, $previousExe)
    $runKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($runPath, $true)
    $runValue = $null
    if ($null -ne $runKey) {
        $runValue = $runKey.GetValue($runName, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    }
    if ($null -ne $runValue -and $runValue.Trim() -ne (CommandFor $destination) -and
        $runValue.Trim() -ne (CommandFor $previous)) {
        throw "Unrelated or stale Run value '$runName'; resolve it before setup."
    }

    if ($ValidateOnly) {
        if ($null -ne $runKey) { $runKey.Close() }
        exit 0
    }

    if ($Action -eq 'Install') {
        $source = Join-Path $PSScriptRoot 'token-ashtray.exe'
        if (-not [IO.File]::Exists($source) -or (Get-Item -LiteralPath $source).Length -eq 0) {
            throw "Missing archive executable: $source"
        }
        [IO.Directory]::CreateDirectory($destination) | Out-Null
        [IO.File]::WriteAllText((Join-Path $destination $markerName), $markerText)
        CopyAtomic $source $currentExe
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($shortcutPath)) | Out-Null
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut($shortcutPath)
        $shortcut.TargetPath = $currentExe
        $shortcut.WorkingDirectory = $destination
        $shortcut.IconLocation = "$currentExe,0"
        $shortcut.Description = $shortcutDescription
        $shortcut.Arguments = ''
        $shortcut.Save()
        if ($null -ne $runValue -and $runValue.Trim() -eq (CommandFor $previous) -and
            -not (SamePath $previous $destination)) {
            $runKey.SetValue($runName, (CommandFor $destination), [Microsoft.Win32.RegistryValueKind]::String)
        }
        if (-not (SamePath $previous $destination)) { RemoveOwnedFiles $previous }
        Write-Output "Installed $currentExe; launch from the Start menu."
    } else {
        if ($null -ne $runValue) { $runKey.DeleteValue($runName, $false) }
        RemoveStartupSwitch $startupApprovedPath $runName
        if ($shortcutExists) { [IO.File]::Delete($shortcutPath) }
        RemoveOwnedFiles $destination
        $settings = Join-Path $env:APPDATA 'rammi\token-ashtray\config\config.toml'
        if ($RemoveSettings) {
            RemoveSettingsFile $settings
            Write-Output "Removed app preferences at $settings; provider CLI credentials were not changed."
        } elseif ([IO.File]::Exists($settings)) { Write-Output "Kept app preferences at $settings" }
    }
    if ($null -ne $runKey) { $runKey.Close() }
} catch {
    if ($null -ne $runKey) { $runKey.Close() }
    [Console]::Error.WriteLine("Token Ashtray setup: $($_.Exception.Message)")
    exit 1
}
