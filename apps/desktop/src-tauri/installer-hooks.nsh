; Installer hooks (Tauri NSIS installerHooks). Decision 010: the installer has two modes.
;
;   All users   C:\Program Files\Volt, written by an elevated installer. The folder keeps the ACL it
;               inherits from Program Files (administrators only can change it), the helper
;               (volt-helper.exe) is installed next to volt-desktop.exe and administrator features
;               can be used.
;   Just me     the user's own folder. A folder the user can write to must never hold the helper,
;               so it is removed again and administrator features stay off.
;
; The installer is the stock Tauri one, so what it does is limited to these hooks. Nothing here
; reads, runs or deletes anything in HKCU, %APPDATA%, %LOCALAPPDATA% or %TEMP%.

; Stops a running helper (all users only). The helper exits by itself when the UI that started it
; goes away; give it 5 seconds, then end it, so the executable can be replaced or removed. $R8 and
; $R9 are scratch.
!macro VoltStopHelper id
  ${If} $MultiUser.InstallMode == "AllUsers"
    StrCpy $R8 0
    volt_helper_wait_${id}:
      nsis_tauri_utils::FindProcess "volt-helper.exe"
      Pop $R9
      ${If} $R9 = 0
        ${If} $R8 < 10
          IntOp $R8 $R8 + 1
          Sleep 500
          Goto volt_helper_wait_${id}
        ${EndIf}
        nsis_tauri_utils::KillProcess "volt-helper.exe"
        Pop $R9
        Sleep 500
      ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ${If} $MultiUser.InstallMode == "AllUsers"
    ; The helper and the app both refuse to use any other folder, so do not install into one.
    ${If} $INSTDIR != "$PROGRAMFILES64\${PRODUCTNAME}"
      MessageBox MB_OK|MB_ICONSTOP "Volt for all users installs only into $PROGRAMFILES64\${PRODUCTNAME}. Choose Just me to install into another folder." /SD IDOK
      Abort
    ${EndIf}
  ${EndIf}
  !insertmacro VoltStopHelper install
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} $MultiUser.InstallMode == "AllUsers"
    ; Give every installed entry the inherited ACL of Program Files (an old folder may carry
    ; another one). The helper's own state folder, when it exists, has its own ACL and is skipped.
    nsExec::Exec '"$SYSDIR\icacls.exe" "$INSTDIR" /reset /C /Q'
    Pop $R9
    FindFirst $R8 $R7 "$INSTDIR\*"
    volt_acl_next:
      StrCmp $R7 "" volt_acl_done
      StrCmp $R7 "." volt_acl_skip
      StrCmp $R7 ".." volt_acl_skip
      StrCmp $R7 "state" volt_acl_skip
      nsExec::Exec '"$SYSDIR\icacls.exe" "$INSTDIR\$R7" /reset /T /C /Q'
      Pop $R9
      volt_acl_skip:
      FindNext $R8 $R7
      Goto volt_acl_next
    volt_acl_done:
    FindClose $R8
  ${Else}
    Delete "$INSTDIR\volt-helper.exe"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro VoltStopHelper uninstall
!macroend
