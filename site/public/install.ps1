# klark installer for Windows (PowerShell 5.1 or 7).
#
#   irm https://muthuishere.github.io/klark/install.ps1 | iex
#
# Downloads klark.exe from the GitHub release, checks its sha256, installs it for your
# user (no admin needed) and adds it to your PATH.
#
#   $env:KLARK_VERSION = "v0.9.5.5"   install that release (default: the latest)
#   $env:INSTALL_DIR = "C:\tools"     where to put it (default: %LOCALAPPDATA%\klark\bin)
#
# The build is x64; it also runs on Windows on ARM. Windows support is new and has had
# little real use: please report problems.
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # Invoke-WebRequest is much faster without the progress bar

$repo = "muthuishere/klark"
$file = "klark-windows-x86_64.exe"
$sumFile = "klark-windows-x86_64.sha256"
if ($env:KLARK_VERSION) {
    $base = "https://github.com/$repo/releases/download/$($env:KLARK_VERSION)"
} else {
    $base = "https://github.com/$repo/releases/latest/download"
}
if ($env:INSTALL_DIR) { $dest = $env:INSTALL_DIR } else { $dest = Join-Path $env:LOCALAPPDATA "klark\bin" }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("klark-install-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    Write-Host "Downloading $file ..."
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest "$base/$file" -OutFile (Join-Path $tmp $file) -UseBasicParsing
    Invoke-WebRequest "$base/$sumFile" -OutFile (Join-Path $tmp $sumFile) -UseBasicParsing

    $want = ((Get-Content (Join-Path $tmp $sumFile) -Raw) -split "\s+")[0].Trim().ToLower()
    $got = (Get-FileHash (Join-Path $tmp $file) -Algorithm SHA256).Hash.ToLower()
    if ($want -ne $got) { throw "checksum mismatch (expected $want, got $got); nothing installed" }

    New-Item -ItemType Directory -Force $dest | Out-Null
    $exe = Join-Path $dest "klark.exe"
    try {
        Copy-Item (Join-Path $tmp $file) $exe -Force
    } catch {
        throw "cannot write $exe. If klark is running, close it (klark server stop) and run this again. $($_.Exception.Message)"
    }
    Write-Host "Installed: $exe"
    & $exe --version

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not (($userPath -split ";") -contains $dest)) {
        $newPath = if ([string]::IsNullOrEmpty($userPath)) { $dest } else { "$userPath;$dest" }
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        $env:Path = "$env:Path;$dest"
        Write-Host "Added $dest to your PATH. Open a new terminal to use 'klark' there."
    }
    Write-Host ""
    Write-Host "Run: klark"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
