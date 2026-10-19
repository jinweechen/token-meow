; Hidden current-user/x64 engine for the independent TokenMeow installer shell.
; This include contains macros only: product defines are supplied by Tauri after
; installer-hooks.nsh is included. No process is ever shut down by this engine.
!ifndef TOKENMEOW_HIDDEN_ENGINE
!define TOKENMEOW_HIDDEN_ENGINE

!define TM_EXIT_SILENT_REQUIRED 64
!define TM_EXIT_APP_RUNNING 65
!define TM_EXIT_PROCESS_PROBE_FAILED 66
!define TM_EXIT_UNSUPPORTED_UPGRADE 67
!define TM_EXIT_DOWNGRADE 68
!define TM_EXIT_WEBVIEW_FAILED 69
Var TM_EngineExit
Var TM_EngineKnownUninstall

!macro TM_ENGINE_REQUIRE_SILENT
  ${IfNot} ${Silent}
    SetErrorLevel ${TM_EXIT_SILENT_REQUIRED}
    Quit
  ${EndIf}
!macroend

; Restart Manager is queried only. An unknown result fails closed, and the
; session is ended before exiting. In particular, never call RmShutdown.
!macro TM_ENGINE_REQUIRE_APP_STOPPED executablePath
  Push $0
  Push $1
  Push $2
  Push $3
  Push $R0
  StrCpy $TM_EngineExit 0
  !insertmacro RestartManager_StartSession $R0
  ${If} $R0 == ""
    StrCpy $TM_EngineExit ${TM_EXIT_PROCESS_PROBE_FAILED}
  ${Else}
    !insertmacro RestartManager_RegisterFile $R0 "${executablePath}"
    ${If} $0 != 0
      StrCpy $TM_EngineExit ${TM_EXIT_PROCESS_PROBE_FAILED}
    ${Else}
      System::Call 'RSTRTMGR::RmGetList(i R0, *i .r1, *i 0r2, p 0, *i .r3) i.r0'
      ${If} $0 = ${ERROR_MORE_DATA}
        StrCpy $TM_EngineExit ${TM_EXIT_APP_RUNNING}
      ${ElseIf} $0 != 0
        StrCpy $TM_EngineExit ${TM_EXIT_PROCESS_PROBE_FAILED}
      ${ElseIf} $1 != 0
      ${OrIf} $2 != 0
        StrCpy $TM_EngineExit ${TM_EXIT_APP_RUNNING}
      ${EndIf}
    ${EndIf}
    System::Call 'RSTRTMGR::RmEndSession(i R0) i.r3'
    ${If} $3 != 0
      StrCpy $TM_EngineExit ${TM_EXIT_PROCESS_PROBE_FAILED}
    ${EndIf}
  ${EndIf}
  Pop $R0
  Pop $3
  Pop $2
  Pop $1
  Pop $0
  ${If} $TM_EngineExit != 0
    ${IfNot} ${Silent}
      MessageBox MB_ICONEXCLAMATION "TokenMeow 仍在运行或无法确认已退出。请先正常退出 TokenMeow，再卸载。"
    ${EndIf}
    SetErrorLevel $TM_EngineExit
    Quit
  ${EndIf}
  ClearErrors
!macroend

; Never delegate to a legacy uninstaller: it may force-close the application or
; display a native wizard. Matching non-owned registrations need manual review.
!macro TM_ENGINE_CHECK_REGISTRATIONS root view ownView
  SetRegView ${view}
  StrCpy $0 0
  ${Do}
    EnumRegKey $1 ${root} "Software\Microsoft\Windows\CurrentVersion\Uninstall" $0
    ${If} $1 == ""
      ${ExitDo}
    ${EndIf}
    IntOp $0 $0 + 1
    ReadRegStr $2 ${root} "Software\Microsoft\Windows\CurrentVersion\Uninstall\$1" "DisplayName"
    ReadRegStr $3 ${root} "Software\Microsoft\Windows\CurrentVersion\Uninstall\$1" "Publisher"
    ${If} $2 == "${PRODUCTNAME}"
    ${AndIf} $3 == "${MANUFACTURER}"
      !if "${ownView}" == "true"
        ${If} $1 != "${PRODUCTNAME}"
          StrCpy $TM_EngineExit ${TM_EXIT_UNSUPPORTED_UPGRADE}
        ${Else}
          ; HKCU software may be shared between registry views. Only the exact
          ; already-known 64-bit registration may appear in both views.
          ReadRegStr $2 ${root} "Software\Microsoft\Windows\CurrentVersion\Uninstall\$1" "UninstallString"
          ${If} $TM_EngineKnownUninstall == ""
          ${OrIf} $2 != $TM_EngineKnownUninstall
            StrCpy $TM_EngineExit ${TM_EXIT_UNSUPPORTED_UPGRADE}
          ${EndIf}
        ${EndIf}
      !else
        StrCpy $TM_EngineExit ${TM_EXIT_UNSUPPORTED_UPGRADE}
      !endif
    ${EndIf}
  ${Loop}
!macroend

!macro TM_ENGINE_VALIDATE_UPGRADE
  !if "${INSTALLMODE}" != "currentUser"
    !error "TokenMeow hidden engine supports currentUser only"
  !endif
  !if "${ARCH}" != "x64"
    !error "TokenMeow hidden engine supports x64 only"
  !endif
  Push $0
  Push $1
  Push $2
  Push $3
  Push $R1
  Push $R2
  StrCpy $TM_EngineExit 0
  SetRegView 64
  ReadRegStr $TM_EngineKnownUninstall HKCU "${UNINSTKEY}" "UninstallString"
  !insertmacro TM_ENGINE_CHECK_REGISTRATIONS HKLM 64 false
  !insertmacro TM_ENGINE_CHECK_REGISTRATIONS HKLM 32 false
  !insertmacro TM_ENGINE_CHECK_REGISTRATIONS HKCU 32 true
  !insertmacro TM_ENGINE_CHECK_REGISTRATIONS HKCU 64 true
  ; SetContext selects the 64-bit current-user view for this product.
  SetRegView 64
  ReadRegStr $0 HKCU "${UNINSTKEY}" "InstallLocation"
  ReadRegStr $1 HKCU "${UNINSTKEY}" "MainBinaryName"
  ReadRegStr $2 HKCU "${UNINSTKEY}" "UninstallString"
  ReadRegStr $3 HKCU "${UNINSTKEY}" "DisplayVersion"
  ReadRegStr $R1 HKCU "${UNINSTKEY}" "DisplayName"
  ReadRegStr $R2 HKCU "${UNINSTKEY}" "Publisher"
  ${If} "$0$1$2$3" == ""
    ; An unregistered existing payload is not a supported in-place upgrade.
    ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    ${OrIf} ${FileExists} "$INSTDIR\uninstall.exe"
    ${OrIf} $R1 != ""
    ${OrIf} $R2 != ""
      StrCpy $TM_EngineExit ${TM_EXIT_UNSUPPORTED_UPGRADE}
    ${EndIf}
  ${Else}
    ${If} $1 S!= "${MAINBINARYNAME}.exe"
    ${OrIf} $0 == ""
    ${OrIf} $2 == ""
    ${OrIf} $3 == ""
    ${OrIf} $R1 != "${PRODUCTNAME}"
    ${OrIf} $R2 != "${MANUFACTURER}"
      StrCpy $TM_EngineExit ${TM_EXIT_UNSUPPORTED_UPGRADE}
    ${Else}
      ; Stock Tauri stores a quoted InstallLocation. Strip only a matching pair.
      StrCpy $R1 $0 1
      StrCpy $R2 $0 1 -1
      ${If} $R1 == '$\"'
      ${AndIf} $R2 == '$\"'
        StrCpy $0 $0 -1 1
      ${ElseIf} $R1 == '$\"'
      ${OrIf} $R2 == '$\"'
        StrCpy $TM_EngineExit ${TM_EXIT_UNSUPPORTED_UPGRADE}
      ${EndIf}
      ClearErrors
      GetFullPathName $R1 "$0"
      ${If} ${Errors}
        StrCpy $TM_EngineExit ${TM_EXIT_UNSUPPORTED_UPGRADE}
      ${EndIf}
      GetFullPathName $R2 "$INSTDIR"
      ${If} ${Errors}
        StrCpy $TM_EngineExit ${TM_EXIT_UNSUPPORTED_UPGRADE}
      ${EndIf}
      ${If} $R1 == ""
      ${OrIf} $R2 == ""
      ${OrIf} $R1 != $R2
      ${OrIf} $2 != '$\"$0\uninstall.exe$\"'
        StrCpy $TM_EngineExit ${TM_EXIT_UNSUPPORTED_UPGRADE}
      ${EndIf}
    ${EndIf}
  ${EndIf}
  Pop $R2
  Pop $R1
  Pop $3
  Pop $2
  Pop $1
  Pop $0
  ${If} $TM_EngineExit != 0
    SetErrorLevel $TM_EngineExit
    Quit
  ${EndIf}
  ClearErrors
!macroend
!endif
