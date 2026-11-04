; Orbit's Windows installer, per user. It takes over the Tauri Orbit's install:
; same folder ($LOCALAPPDATA\Orbit, or where that one was put), Orbit.exe,
; Start menu entry and uninstall key, so its updater can run this one in place.
; makensis -DVERSION=1.2.3 -DEXE=Orbit.exe -DICON=icon.ico -DOUTFILE=setup.exe installer.nsi
; Flags (the Tauri updater's): /S or /P silent, /R start Orbit when done.
Unicode true
ManifestDPIAware true
SetCompressor /SOLID lzma
RequestExecutionLevel user

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

; The Tauri installer's keys (publisher: the identifier's second part).
!define MANUFACTURER "nsx001220"
!define MANUPRODUCTKEY "Software\${MANUFACTURER}\Orbit"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Orbit"

Name "Orbit"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Orbit"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_FINISHPAGE_RUN "$INSTDIR\Orbit.exe"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Var Relaunch

Function .onInit
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/P" $R1
  ${IfNot} ${Errors}
    SetSilent silent
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/R" $R1
  ${IfNot} ${Errors}
    StrCpy $Relaunch 1
  ${EndIf}
  ReadRegStr $R2 HKCU "${MANUPRODUCTKEY}" ""
  ${If} $R2 != ""
    StrCpy $INSTDIR $R2
  ${EndIf}
FunctionEnd

; A Tauri Orbit installed from its .msi (per machine): uninstall it first.
Function RemoveMsiInstall
  StrCpy $R0 0
  ${Do}
    EnumRegKey $R1 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall" $R0
    ${If} $R1 == ""
      ${Break}
    ${EndIf}
    IntOp $R0 $R0 + 1
    ReadRegStr $R2 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\$R1" "DisplayName"
    ReadRegStr $R3 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\$R1" "Publisher"
    ReadRegDWORD $R4 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\$R1" "WindowsInstaller"
    ${If} $R2 == "Orbit"
    ${AndIf} $R3 == "${MANUFACTURER}"
    ${AndIf} $R4 == 1
      ExecWait 'msiexec.exe /x $R1 /passive /norestart'
      ${Break}
    ${EndIf}
  ${Loop}
FunctionEnd

Section "Orbit"
  Call RemoveMsiInstall
  SetOutPath "$INSTDIR"
  ; The app that started this installer may still be exiting.
  ${For} $R2 1 20
    Delete "$INSTDIR\Orbit.exe"
    ${IfNot} ${FileExists} "$INSTDIR\Orbit.exe"
      ${Break}
    ${EndIf}
    Sleep 500
  ${Next}
  Delete "$INSTDIR\Orbit.exe.old"
  File "/oname=Orbit.exe" "${EXE}"
  File "/oname=Orbit.ico" "${ICON}"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  ; Kept when there (the Tauri app's points at the same exe).
  ${IfNot} ${FileExists} "$SMPROGRAMS\Orbit.lnk"
    CreateShortCut "$SMPROGRAMS\Orbit.lnk" "$INSTDIR\Orbit.exe" "" "$INSTDIR\Orbit.ico"
  ${EndIf}
  WriteRegStr HKCU "${MANUPRODUCTKEY}" "" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "MainBinaryName" "Orbit.exe"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "Orbit"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" '"$INSTDIR\Orbit.exe"'
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" '"$INSTDIR"'
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTKEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
  ${If} $Relaunch == 1
  ${AndIf} ${Silent}
    Exec '"$INSTDIR\Orbit.exe"'
  ${EndIf}
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\Orbit.exe"
  Delete "$INSTDIR\Orbit.exe.old"
  Delete "$INSTDIR\Orbit.ico"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Orbit.lnk"
  Delete "$DESKTOP\Orbit.lnk"
  DeleteRegKey HKCU "${UNINSTKEY}"
  DeleteRegKey HKCU "${MANUPRODUCTKEY}"
SectionEnd
