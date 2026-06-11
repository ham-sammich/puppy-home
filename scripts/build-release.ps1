# Build a distributable Windows release of puppy-home (+ the browser plugin),
# laid out exactly how plugin discovery expects it:
#
#   dist/puppy-home-windows/
#     puppy-home.exe
#     plugins/browser/plugin.json
#     plugins/browser/puppy-browser.exe
#
# Run from the repo root:  powershell -File scripts/build-release.ps1
$ErrorActionPreference = "Stop"

Write-Host "Building release (workspace: app + browser plugin)..."
cargo build --release --workspace
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$dist = "dist/puppy-home-windows"
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory -Force "$dist/plugins/browser" | Out-Null

Copy-Item "target/release/puppy-home.exe" "$dist/"
Copy-Item "target/release/puppy-browser.exe" "$dist/plugins/browser/"

@"
{
  "id": "browser",
  "name": "Web Browser",
  "version": "1.0.0",
  "exe": "puppy-browser.exe",
  "min_host_version": "0.0.0"
}
"@ | Set-Content "$dist/plugins/browser/plugin.json"

Compress-Archive -Path $dist -DestinationPath "dist/puppy-home-windows.zip" -Force

Write-Host ""
Write-Host "Done."
Write-Host "  Folder : $dist"
Write-Host "  Zip    : dist/puppy-home-windows.zip"
Write-Host ""
Write-Host "Runtime requirements on the target machine: python + code_puppy"
Write-Host "(pip install code-puppy) and WebView2 (preinstalled on Win 10/11)."
