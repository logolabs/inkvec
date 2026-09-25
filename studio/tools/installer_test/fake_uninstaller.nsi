; A stand-in for an old "Inkvec Studio Lite" uninstaller -- the .exe's uninstall.exe, and
; msiexec for the .msi -- for the installer hooks' dry test
; (studio/tools/check_installer_hooks.py). It does, silently, what the real ones do to the
; things the hooks care about: logs how it was called, removes the app's files and its
; Apps entry, and (the .exe's) runs the old app's PREUNINSTALL, which removes the context
; menu and the command -- all under the test's own registry key and folder, never the
; machine's real ones.
Unicode true
RequestExecutionLevel user
SilentInstall silent
OutFile "${OUT}"
!include LogicLib.nsh
!include StrFunc.nsh
${StrLoc}

!define UNINSTALL "${INKVEC_SOFTWARE}\Microsoft\Windows\CurrentVersion\Uninstall"

Section
  FileOpen $0 "${WORK}\uninstall-called.txt" a
  FileSeek $0 0 END
  FileWrite $0 "$CMDLINE$\r$\n"
  FileClose $0
  ${StrLoc} $1 $CMDLINE "/passive" ">"
  ${If} $1 != ""
    ; As msiexec /x {product code}: the entry goes; the context menu is left as it was.
    DeleteRegKey HKCU "${UNINSTALL}\${DRY_GUID}"
  ${Else}
    ; As the .exe's uninstaller, which lives in the old install's folder.
    Delete "$EXEDIR\inkvec-hook-dry-app.exe"
    DeleteRegKey HKCU "${UNINSTALL}\Inkvec Studio Lite"
    DeleteRegKey HKCU "${INKVEC_SOFTWARE}\Classes\SystemFileAssociations\.png\shell\InkvecStudio"
    DeleteRegKey HKCU "${INKVEC_SOFTWARE}\Classes\SystemFileAssociations\.tif\shell\InkvecStudio"
    Delete "${INKVEC_CLI_DIR}\inkvec.exe"
  ${EndIf}
SectionEnd
