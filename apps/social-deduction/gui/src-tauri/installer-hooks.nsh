; The NSIS installer's hooks (bundle > windows > nsis > installerHooks in
; tauri.conf.json). Installing puts the install folder, which holds
; social-deduction.exe, on the user's Path, and uninstalling takes it off.
; user-path.ps1, which every app shares from crates/desktop, edits the
; Path, and each terminal opened afterward sees it.

; Here rather than in the macro, where __FILEDIR__ is installer.nsi's.
; Tauri includes this file where it is, so __FILEDIR__ is src-tauri.
!define USER_PATH_SCRIPT "${__FILEDIR__}\..\..\..\..\crates\desktop\user-path.ps1"

!macro USER_PATH ACTION
  InitPluginsDir
  File "/oname=$PLUGINSDIR\user-path.ps1" "${USER_PATH_SCRIPT}"
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\user-path.ps1" -${ACTION} "$INSTDIR"'
  Pop $0
  DetailPrint "user-path.ps1 -${ACTION}: $0"
  ; WM_SETTINGCHANGE to every top-level window, Explorer among them.
  SendMessage 0xFFFF 0x001A 0 "STR:Environment" /TIMEOUT=5000
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro USER_PATH Add
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro USER_PATH Remove
!macroend
