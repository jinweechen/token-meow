; TokenMeow hooks for the pinned Tauri CLI 2.12.3 stock NSIS template.
; The application is currentUser/x64. Never execute a value read from Run.
; Keep capture before the stock reinstall page can invoke the old uninstaller.
Var TM_RunCaptured
Var TM_PreviousRun
Var TM_PreviousRunOwned
Var TM_RunReadStatus
Var TM_RunReadValue

; Read value, type and existence together. A custom empty/binary/expandable
; value must never be mistaken for a deleted entry during an upgrade.
; RRF_RT_REG_SZ | RRF_NOEXPAND | RRF_ZEROONFAILURE. Only status 2 means absent.
!define /math TM_RUN_BUFFER_BYTES ${NSIS_MAX_STRLEN} * 2
!macro TM_READ_RUN
  Push $0
  Push $1
  StrCpy $0 ""
  StrCpy $1 -1
  System::Call 'advapi32::RegGetValueW(p 0x80000001, w "Software\Microsoft\Windows\CurrentVersion\Run", w "TokenMeow", i 0x30000002, p 0, w .r0, *i ${TM_RUN_BUFFER_BYTES}) i.r1'
  StrCpy $TM_RunReadValue $0
  StrCpy $TM_RunReadStatus $1
  Pop $1
  Pop $0
!macroend

!define MUI_CUSTOMFUNCTION_GUIINIT TM_CaptureUpgradeRun

Function TM_CaptureUpgradeRun
  Push $0
  Push $1
  Push $2
  ${If} $TM_RunCaptured != 1
    StrCpy $TM_RunCaptured 1
    StrCpy $TM_PreviousRunOwned 0
    !insertmacro TM_READ_RUN
    StrCpy $TM_PreviousRun $TM_RunReadValue
    ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\TokenMeow" "InstallLocation"
    ReadRegStr $1 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\TokenMeow" "MainBinaryName"
    ${If} $0 != ""
    ${AndIf} $1 S== "token-meow.exe"
    ${AndIf} $TM_RunReadStatus = 0
      ; Stock Tauri writes InstallLocation surrounded by quotes.
      StrCpy $2 $0 1
      ${If} $2 == '$\"'
        StrCpy $0 $0 -1 1
      ${EndIf}
      StrCpy $2 '$\"$0\token-meow.exe$\" --background'
      ${If} $TM_PreviousRun S== $2
        StrCpy $TM_PreviousRunOwned 1
      ${EndIf}
    ${EndIf}
  ${EndIf}
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  ; Silent mode does not initialize a GUI; stock silent install has no
  ; reinstall page, so capture here still precedes any replacement.
  Call TM_CaptureUpgradeRun
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Push $0
  ${If} $TM_PreviousRunOwned = 1
    !insertmacro TM_READ_RUN
    StrCpy $0 0
    ${If} $TM_RunReadStatus = 2
      StrCpy $0 1
    ${ElseIf} $TM_RunReadStatus = 0
    ${AndIf} $TM_RunReadValue S== $TM_PreviousRun
      StrCpy $0 1
    ${EndIf}
    ${If} $0 = 1
      WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "TokenMeow" '$\"$INSTDIR\${MAINBINARYNAME}.exe$\" --background'
    ${EndIf}
  ${EndIf}
  Pop $0
!macroend

