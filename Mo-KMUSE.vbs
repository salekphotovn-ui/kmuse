Option Explicit
Dim shell, fs, app
Set shell = CreateObject("WScript.Shell")
Set fs = CreateObject("Scripting.FileSystemObject")
app = fs.BuildPath(fs.GetParentFolderName(WScript.ScriptFullName), "KMUSE.exe")
If fs.FileExists(app) Then
  shell.Run Chr(34) & app & Chr(34), 1, False
Else
  MsgBox "Khong tim thay KMUSE.exe trong thu muc tool.", 16, "KMUSE"
End If
