@echo off
setlocal
set "APP=%~dp0KMUSE.exe"
if not exist "%APP%" (
  powershell -NoProfile -WindowStyle Hidden -Command "Add-Type -AssemblyName PresentationFramework; [System.Windows.MessageBox]::Show('Khong tim thay Muse Tool. Hay build ung dung truoc.','Muse Tool') | Out-Null"
  exit /b 1
)
start "" "%APP%"
exit /b 0
