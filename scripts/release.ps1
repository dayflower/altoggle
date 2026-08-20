# Builds and packages a release.
#
# The point of this script is not convenience, it is that "ship altoggle.exe and
# nothing else" becomes executable rather than remembered. `cargo build
# --release` produces four binaries: altoggle, and three developer tools. One of
# them is called keylog.exe, and shipping it next to a tray app would be an
# antivirus incident and a trust problem in one.
#
# Deliberately ASCII-only: Windows PowerShell 5.1 reads a BOM-less script as
# ANSI, and a stray non-ASCII character would arrive mangled.
#
# Two things come out of dist\, because the two package managers want
# different shapes of the same binary:
#
#   altoggle-v<version>-x86_64-pc-windows-msvc.zip   humans, and the Scoop
#                                                    manifest, which extracts it
#   altoggle.exe                                     the winget manifest, which
#                                                    is InstallerType: portable
#
# It is one build either way; the exe in the zip and the exe beside it are the
# same file. Both SHA256s are printed because a manifest asks for one and the
# release notes publish both.

$ErrorActionPreference = 'Stop'

$root = Split-Path $PSScriptRoot -Parent
$manifest = Join-Path $root 'Cargo.toml'

$version = (Select-String -Path $manifest -Pattern '^version = "(.+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $version) { throw "could not read the version out of $manifest" }

# A running altoggle holds target\release\altoggle.exe open and cargo cannot
# replace it. Cargo's own report is "Access is denied. (os error 5)" against a
# path, which sends you looking at permissions. Say the real reason first, and
# do not kill it: the user may be typing Japanese with it right now.
$running = Get-Process -Name 'altoggle' -ErrorAction SilentlyContinue
if ($running) {
    throw ("altoggle is running (pid {0}); cargo cannot replace the executable. " +
        "Quit it from the tray, then run this again." -f ($running.Id -join ', '))
}

Write-Host "Building altoggle $version..."
& cargo build --release --bin altoggle
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

$exe = Join-Path $root 'target\release\altoggle.exe'
if (-not (Test-Path $exe)) { throw "the build produced no $exe" }

# Rebuilt from scratch every time, so a file removed from the list below cannot
# survive in the zip from an earlier run.
$dist = Join-Path $root 'dist'
$stage = Join-Path $dist 'altoggle'
if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

# The whole allowlist. altoggle.exe is self-contained: the icons are compiled
# into it and there is nothing else to carry.
Copy-Item $exe $stage
Copy-Item (Join-Path $root 'README.md') $stage
Copy-Item (Join-Path $root 'LICENSE') $stage

# The name the Scoop manifest points at, so it carries the version and the
# target triple. Scoop autoupdate rebuilds it for each new version, which is
# why the shape matters more than the prettiness.
$zip = Join-Path $dist "altoggle-v$version-x86_64-pc-windows-msvc.zip"
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
Remove-Item $stage -Recurse -Force

# The bare exe, beside the zip rather than inside it. A winget portable package
# downloads exactly one file and links it onto PATH, with nowhere to unpack an
# archive to; pointing winget at the zip instead would mean NestedInstallerType
# and buy nothing. Unversioned on purpose: the download URL already carries the
# tag, and this name is the one winget leaves on disk.
$exeAsset = Join-Path $dist 'altoggle.exe'
Copy-Item $exe $exeAsset

Write-Host ''
foreach ($asset in @($zip, $exeAsset)) {
    $hash = (Get-FileHash $asset -Algorithm SHA256).Hash
    $size = [math]::Round((Get-Item $asset).Length / 1KB)
    Write-Host "  $asset"
    Write-Host "  $size KB"
    Write-Host "  SHA256 $hash"
    Write-Host ''
}
Write-Host "Contents of the zip:"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($zip)
try { $archive.Entries | ForEach-Object { Write-Host "  $($_.FullName)" } }
finally { $archive.Dispose() }
Write-Host ''
Write-Host "This is what CI packages too. To release, run scripts\bump.ps1 and merge the PR."
