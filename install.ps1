# Installs the latest Novo release on Windows:
#   irm https://raw.githubusercontent.com/Simonf8/novo/master/install.ps1 | iex
# Set $env:NOVO_VERSION (e.g. v0.2.0) to pick a release and $env:NOVO_HOME to
# choose where it goes (default ~\.novo; the executable lands in its bin folder).
$ErrorActionPreference = 'Stop'

$repo = 'Simonf8/novo'
$home_dir = if ($env:NOVO_HOME) { $env:NOVO_HOME } else { Join-Path $HOME '.novo' }
$version = $env:NOVO_VERSION
if (-not $version) {
    $version = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
}

$name = "novo-$version-x86_64-pc-windows-msvc"
$base = "https://github.com/$repo/releases/download/$version"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
try {
    Write-Host "Downloading Novo $version"
    Invoke-WebRequest "$base/$name.zip" -OutFile "$tmp\$name.zip"
    Invoke-WebRequest "$base/SHA256SUMS" -OutFile "$tmp\SHA256SUMS"
    $line = Get-Content "$tmp\SHA256SUMS" | Where-Object { $_ -match " $([regex]::Escape("$name.zip"))$" }
    $expected = ($line -split ' ')[0]
    $actual = (Get-FileHash "$tmp\$name.zip" -Algorithm SHA256).Hash.ToLower()
    if (-not $expected -or $expected -ne $actual) { throw "checksum mismatch for $name.zip" }

    Expand-Archive "$tmp\$name.zip" -DestinationPath $tmp
    $bin = Join-Path $home_dir 'bin'
    New-Item -ItemType Directory -Force $bin | Out-Null
    Copy-Item "$tmp\$name\novo.exe" (Join-Path $bin 'novo.exe') -Force
    Write-Host "Installed to $bin\novo.exe"

    $path = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($path -split ';') -notcontains $bin) {
        [Environment]::SetEnvironmentVariable('Path', "$bin;$path", 'User')
        Write-Host "Added $bin to your PATH; open a new terminal to use novo."
    }
} finally {
    Remove-Item -Recurse -Force $tmp
}
