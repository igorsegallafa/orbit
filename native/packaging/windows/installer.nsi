; Orbit's Windows installer: per user, into %LOCALAPPDATA%\Orbit (where the
; Tauri app's NSIS installer put Orbit.exe, so an update from it lands in place).
; makensis -DVERSION=1.2.3 -DEXE=Orbit.exe -DICON=icon.ico -DOUTFILE=setup.exe installer.nsi
; Flags (the Tauri updater's): /S or /P silent, /R start Orbit when done.
Unicode true
ManifestDPIAware true
SetCompressor /SOLID lzma
RequestExecutionLevel user

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

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
FunctionEnd

Section "Orbit"
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
  CreateShortCut "$SMPROGRAMS\Orbit.lnk" "$INSTDIR\Orbit.exe" "" "$INSTDIR\Orbit.ico"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "Orbit"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "Igor Segalla"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\Orbit.ico"
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
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
  DeleteRegKey HKCU "${UNINSTKEY}"
SectionEnd
