# Bumps the version, and hands the rest to CI.
#
# The version has two homes in the tree -- Cargo.toml and crates\app\app.rc,
# because rc.exe cannot read Cargo.toml -- and it used to have a third in the
# git tag, which nothing checked. This script is what makes the number typed
# once: it writes both files, proves they agree by running the test that
# compares them, and opens the pull request. The tag is never typed at all.
# .github\workflows\release.yml derives it from Cargo.toml after the merge, so
# a tag disagreeing with the binary is not a thing that can happen rather than
# a thing that gets caught.
#
# Deliberately ASCII-only, for the same reason as release.ps1: Windows
# PowerShell 5.1 reads a BOM-less script as ANSI, and a stray non-ASCII
# character would arrive mangled.
#
# It leaves you on the release branch. Everything up to the commit is
# recoverable with `git switch -` and `git branch -D release/v<version>`.

[CmdletBinding()]
param(
    # major, minor, patch, or an explicit X.Y.Z.
    [Parameter(Mandatory = $true, Position = 0)]
    [string] $Version,

    # Run every check and print every change, then touch nothing.
    [switch] $DryRun
)

$ErrorActionPreference = 'Stop'

$root = Split-Path $PSScriptRoot -Parent
$manifest = Join-Path $root 'Cargo.toml'
$rc = Join-Path $root 'crates\app\app.rc'

# The same line release.ps1 reads, and the first `version = ` in the file: the
# one in [workspace.package], which every crate inherits through
# `version.workspace = true`.
$versionLine = [regex] '(?m)^version = "(.+)"'

# ReadAllText, not `Get-Content -Raw`: Windows PowerShell 5.1 decodes a
# BOM-less file as ANSI, and Cargo.toml has em dashes in its comments. Reading
# them as ANSI and writing them back as UTF-8 is what mangled them once already
# (see the commit that repaired them). .NET's ReadAllText assumes UTF-8.
$manifestText = [System.IO.File]::ReadAllText($manifest)
$found = $versionLine.Match($manifestText)
if (-not $found.Success) { throw "could not read the version out of $manifest" }
$current = $found.Groups[1].Value

$parsed = [regex]::Match($current, '^(\d+)\.(\d+)\.(\d+)$')
if (-not $parsed.Success) {
    throw "the version in $manifest is '$current', not X.Y.Z; cannot bump it"
}
$major = [int] $parsed.Groups[1].Value
$minor = [int] $parsed.Groups[2].Value
$patch = [int] $parsed.Groups[3].Value

# switch -Regex is case-insensitive unless told otherwise, so Patch and PATCH
# resolve too. Anything that is neither a keyword nor X.Y.Z is an error rather
# than something to interpret generously.
$next = switch -Regex ($Version) {
    '^major$' { "$($major + 1).0.0"; break }
    '^minor$' { "$major.$($minor + 1).0"; break }
    '^patch$' { "$major.$minor.$($patch + 1)"; break }
    '^v\d' {
        throw "give the version without the leading v; the tag is CI's to write"
    }
    # rc.exe wants four numbers in FILEVERSION, and the drift test in
    # crates\app\src\lib.rs builds them by replacing the dots with commas. A
    # hyphen survives neither. This is the resource format refusing, not a
    # feature left undone.
    '^\d+\.\d+\.\d+-' {
        throw "app.rc's FILEVERSION cannot express a pre-release; use X.Y.Z"
    }
    '^\d+\.\d+\.\d+$' { $Version; break }
    default {
        throw "expected major, minor, patch or X.Y.Z, got '$Version'"
    }
}

if ([version] $next -le [version] $current) {
    throw "$next does not come after the current version $current"
}

$tag = "v$next"
$branch = "release/$tag"
$zipName = "altoggle-$tag-x86_64-pc-windows-msvc.zip"

Write-Host "altoggle $current -> $next  (tag $tag)"
Write-Host ''

# Checked before anything is written, because noticing at the end -- with a
# branch already pushed -- is the expensive way to find out.
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    throw "gh is not on PATH; this script needs it to open the pull request"
}

$dirty = & git -C $root status --porcelain
if ($LASTEXITCODE -ne 0) { throw "git status failed with exit code $LASTEXITCODE" }
if ($dirty) {
    throw ("the working tree is not clean; commit or stash first:" +
        [Environment]::NewLine + ($dirty -join [Environment]::NewLine))
}

# The branch is cut from origin/main rather than from HEAD, so it does not
# matter which branch -- or which worktree -- this is run from.
& git -C $root fetch origin --quiet
if ($LASTEXITCODE -ne 0) { throw "git fetch origin failed with exit code $LASTEXITCODE" }

if (& git -C $root tag --list $tag) {
    throw "the tag $tag already exists locally"
}
if (& git -C $root ls-remote --tags origin $tag) {
    throw "the tag $tag already exists on origin"
}
if (& git -C $root branch --list $branch) {
    throw "the branch $branch already exists locally"
}
if (& git -C $root ls-remote --heads origin $branch) {
    throw "the branch $branch already exists on origin"
}

# Both files are prepared in memory and reported before either is written, so
# a version string that does not appear where it is expected fails with
# nothing half-changed on disk.
$manifestNext = $versionLine.Replace($manifestText, "version = `"$next`"", 1)

$numericCurrent = ($current -replace '\.', ',') + ',0'
$numericNext = ($next -replace '\.', ',') + ',0'

# The whole of app.rc's copy of the version, written once as templates so the
# before and the after cannot drift apart: {0} is X.Y.Z, {1} the four numbers
# rc.exe wants. The drift test asserts all four fields.
$templates = @(
    'FILEVERSION {1}'
    'PRODUCTVERSION {1}'
    'VALUE "FileVersion", "{0}"'
    'VALUE "ProductVersion", "{0}"'
)
$edits = foreach ($template in $templates) {
    @{
        Old = $template -f $current, $numericCurrent
        New = $template -f $next, $numericNext
    }
}

$rcText = [System.IO.File]::ReadAllText($rc)
$rcNext = $rcText
foreach ($edit in $edits) {
    $hits = [regex]::Matches($rcNext, [regex]::Escape($edit.Old)).Count
    if ($hits -ne 1) {
        throw "expected exactly one '$($edit.Old)' in $rc, found $hits"
    }
    $rcNext = $rcNext.Replace($edit.Old, $edit.New)
}

Write-Host "  $manifest"
Write-Host "    version = `"$current`"  ->  version = `"$next`""
Write-Host "  $rc"
foreach ($edit in $edits) {
    Write-Host "    $($edit.Old)  ->  $($edit.New)"
}
Write-Host "  Cargo.lock is refreshed by the cargo test below"
Write-Host ''
Write-Host "  branch  $branch"
Write-Host "  tag     $tag (written by CI after the merge)"
Write-Host "  asset   $zipName (built by CI after the merge)"
Write-Host ''

if ($DryRun) {
    Write-Host "Dry run: nothing was written, and no branch was created."
    return
}

& git -C $root switch --create $branch origin/main
if ($LASTEXITCODE -ne 0) { throw "git switch failed with exit code $LASTEXITCODE" }

# WriteAllText, not Set-Content: .gitattributes enforces LF and Set-Content
# would write CRLF. The text read above still carries the line endings the file
# had, and WriteAllText's default encoding is UTF-8 without a BOM, which is what
# these two files are.
[System.IO.File]::WriteAllText($manifest, $manifestNext)
[System.IO.File]::WriteAllText($rc, $rcNext)

# This is the step that proves app.rc was not missed -- the drift test in
# crates\app\src\lib.rs compares it against CARGO_PKG_VERSION -- and it
# re-resolves the workspace, which is what updates Cargo.lock's three copies of
# the number. One command doing both is why there is no `cargo update` here.
Write-Host "Running cargo test..."
& cargo test --manifest-path $manifest
if ($LASTEXITCODE -ne 0) {
    throw ("cargo test failed with exit code $LASTEXITCODE; the bumped files " +
        "are still in the working tree on $branch")
}

& git -C $root add -- Cargo.toml Cargo.lock crates/app/app.rc
if ($LASTEXITCODE -ne 0) { throw "git add failed with exit code $LASTEXITCODE" }

& git -C $root commit -m "chore: release $tag"
if ($LASTEXITCODE -ne 0) { throw "git commit failed with exit code $LASTEXITCODE" }

& git -C $root push --set-upstream origin $branch
if ($LASTEXITCODE -ne 0) { throw "git push failed with exit code $LASTEXITCODE" }

$body = @"
Bumps the version to $next.

Merging this cuts the release: release.yml sees the version change on main,
runs the tests, builds altoggle.exe through scripts/release.ps1, and publishes
$zipName as $tag with its SHA256 in the notes.

No tag needs pushing by hand; CI derives it from Cargo.toml.
"@

# gh reads the repository from the working directory, so it gets one.
Push-Location $root
try {
    & gh pr create --base main --head $branch --title "chore: release $tag" --body $body
    if ($LASTEXITCODE -ne 0) { throw "gh pr create failed with exit code $LASTEXITCODE" }
}
finally {
    Pop-Location
}

Write-Host ''
Write-Host "Next: merge that pull request. The release publishes itself."
