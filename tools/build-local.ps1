# Build KMUSE.exe để dùng trên máy này. LUÔN dùng lệnh này thay cho "cargo build --release":
# chỉ "tauri build" mới nhúng sẵn giao diện vào file exe. Build bằng cargo thường sẽ cho ra
# file mở lên chỉ thấy "localhost refused to connect".
# Cách dùng: powershell -File tools/build-local.ps1   (chạy sau khi đã ra bản mới bằng tools/release.mjs)
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
Get-Process KMUSE -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2
npx tauri build --no-bundle
if ($LASTEXITCODE -ne 0) { throw "Build thất bại" }
Copy-Item src-tauri\target\release\muse-tool.exe KMUSE.exe -Force
Write-Host "KMUSE.exe phiên bản $((Get-Item KMUSE.exe).VersionInfo.ProductVersion)"
