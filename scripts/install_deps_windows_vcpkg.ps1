Param(
  [string]$VcpkgDir = "$PSScriptRoot\..\..\vcpkg"
)

$ErrorActionPreference = "Stop"

if (!(Test-Path $VcpkgDir)) {
  git clone https://github.com/microsoft/vcpkg.git $VcpkgDir
}

Push-Location $VcpkgDir
try {
  .\bootstrap-vcpkg.bat
  .\vcpkg.exe integrate install
  .\vcpkg.exe install lua:x64-windows
}
finally {
  Pop-Location
}

Write-Host "Dependencies installed via vcpkg. Use CMake with -DCMAKE_TOOLCHAIN_FILE=$VcpkgDir\scripts\buildsystems\vcpkg.cmake"
