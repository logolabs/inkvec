; Dry test of studio/src-tauri/installer/hooks.nsh, run by studio/tools/check_installer_hooks.py.
;
; It inserts the hooks exactly as Tauri's NSIS template does (PREINSTALL, then POSTINSTALL,
; inside a section, with $INSTDIR set) around a staged "old install", and writes one line
; per check to ${RESULT}. INKVEC_SOFTWARE and INKVEC_CLI_DIR point the hooks at a private
; registry key and folder, so nothing real is read or changed: no uninstall entry, context
; menu or PATH of this machine. The staged uninstaller is fake_uninstaller.nsi.
Unicode true
RequestExecutionLevel user
SilentInstall silent
OutFile "${OUT}"

!include LogicLib.nsh
!include StrFunc.nsh
; As the template declares them before it includes the hooks.
${StrCase}
${StrLoc}

!addplugindir "${PLUGINS}"

; What Tauri's template defines before it inserts the hooks.
!define PRODUCTNAME "Inkvec Studio"
!define MAINBINARYNAME "inkvec-studio"
; INKVEC_SOFTWARE and INKVEC_CLI_DIR come from the command line (check_installer_hooks.py),
; with WORK, DRY_GUID, FAKE, HOOKS, PLUGINS, RESULT and OUT, the same for the fake.
; The machine hive is the test's own key too, and msiexec is the fake.
!define INKVEC_MACHINE HKCU
!define INKVEC_MSIEXEC "${FAKE}"
!include "${HOOKS}"

!define OLDKEY "${INKVEC_SOFTWARE}\Microsoft\Windows\CurrentVersion\Uninstall\Inkvec Studio Lite"
!define MENU "${INKVEC_SOFTWARE}\Classes\SystemFileAssociations"
!define OLD "${WORK}\Inkvec Studio Lite"
; A binary name nothing on this machine runs: the hook closes a running old app, and the
; test must never find (and close) a real Inkvec Studio.
!define DRY_APP "inkvec-hook-dry-app.exe"

!macro CHECK NAME
  ${If} $R0 == "yes"
    FileWrite $9 "ok   ${NAME}$\r$\n"
  ${Else}
    FileWrite $9 "FAIL ${NAME} ($R0)$\r$\n"
  ${EndIf}
!macroend

!macro FILE PATH TEXT
  FileOpen $0 "${PATH}" w
  FileWrite $0 "${TEXT}"
  FileClose $0
!macroend

!macro FRESH
  DeleteRegKey HKCU "${INKVEC_SOFTWARE}"
  RMDir /r "${WORK}"
  CreateDirectory "${WORK}\WindowsApps"
  StrCpy $INSTDIR "${WORK}\Inkvec Studio"
  CreateDirectory "$INSTDIR"
  !insertmacro FILE "$INSTDIR\inkvec.exe" "new command"
  !insertmacro FILE "$INSTDIR\inkvec-studio.exe" "new app"
!macroend

!macro OLD_INSTALL
  CreateDirectory "${OLD}"
  CopyFiles /SILENT "${FAKE}" "${OLD}\uninstall.exe"
  !insertmacro FILE "${OLD}\${DRY_APP}" "old app"
  WriteRegStr HKCU "${OLDKEY}" "DisplayName" "Inkvec Studio Lite"
  WriteRegStr HKCU "${OLDKEY}" "UninstallString" '"${OLD}\uninstall.exe"'
  WriteRegStr HKCU "${OLDKEY}" "MainBinaryName" "${DRY_APP}"
  WriteRegStr HKCU "${INKVEC_SOFTWARE}\LogoLabs\Inkvec Studio Lite" "" "${OLD}"
!macroend

Section
  SetRegView 64
  FileOpen $9 "${RESULT}" w

  ; ---- 1. An old install, with both integrations on --------------------------------
  !insertmacro FRESH
  !insertmacro OLD_INSTALL
  WriteRegStr HKCU "${MENU}\.png\shell\InkvecStudio\command" "" '"${OLD}\inkvec-studio.exe" "%1"'
  WriteRegStr HKCU "${MENU}\.tif\shell\InkvecStudio\command" "" '"${OLD}\inkvec-studio.exe" "%1"'
  !insertmacro FILE "${WORK}\WindowsApps\inkvec.exe" "old command"

  !insertmacro NSIS_HOOK_PREINSTALL
  !insertmacro NSIS_HOOK_POSTINSTALL

  StrCpy $R0 "no log"
  ${If} ${FileExists} "${WORK}\uninstall-called.txt"
    FileOpen $0 "${WORK}\uninstall-called.txt" r
    FileRead $0 $1
    FileClose $0
    StrCpy $R0 "called as: $1"
    ${StrLoc} $2 $1 "/S" ">"
    ${StrLoc} $3 $1 "_?=${OLD}" ">"
    ${If} $2 != ""
    ${AndIf} $3 != ""
      StrCpy $R0 "yes"
    ${EndIf}
  ${EndIf}
  !insertmacro CHECK "the old uninstaller ran silently, in place (/S _?=dir)"

  StrCpy $R0 "yes"
  ${IfThen} ${FileExists} "${OLD}\*.*" ${|} StrCpy $R0 "the old folder is still there" ${|}
  !insertmacro CHECK "the old folder is gone"

  ReadRegStr $1 HKCU "${OLDKEY}" "UninstallString"
  StrCpy $R0 "yes"
  ${IfThen} $1 != "" ${|} StrCpy $R0 "entry still there: $1" ${|}
  !insertmacro CHECK "the old Apps entry is gone"

  ReadRegStr $1 HKCU "${INKVEC_SOFTWARE}\LogoLabs\Inkvec Studio Lite" ""
  StrCpy $R0 "yes"
  ${IfThen} $1 != "" ${|} StrCpy $R0 "still there: $1" ${|}
  !insertmacro CHECK "the old install-location key is gone"

  ReadRegStr $1 HKCU "${MENU}\.png\shell\InkvecStudio\command" ""
  ReadRegStr $2 HKCU "${MENU}\.jpeg\shell\InkvecStudio\command" ""
  StrCpy $R0 "png: $1 / jpeg: $2"
  ${If} $1 == '"$INSTDIR\inkvec-studio.exe" "%1"'
  ${AndIf} $2 == $1
    StrCpy $R0 "yes"
  ${EndIf}
  !insertmacro CHECK "the context menu is back, pointing at the new app"

  StrCpy $R0 "missing"
  ${If} ${FileExists} "${WORK}\WindowsApps\inkvec.exe"
    FileOpen $0 "${WORK}\WindowsApps\inkvec.exe" r
    FileRead $0 $1
    FileClose $0
    StrCpy $R0 "holds: $1"
    ${IfThen} $1 == "new command" ${|} StrCpy $R0 "yes" ${|}
  ${EndIf}
  !insertmacro CHECK "the inkvec command is back, the new app's copy"

  StrCpy $R0 "yes"
  ${IfNot} ${FileExists} "$INSTDIR\inkvec-studio.exe"
    StrCpy $R0 "the new app's files were touched"
  ${EndIf}
  !insertmacro CHECK "the new install is untouched"

  ; ---- 2. No old install, no integrations: nothing happens -----------------------------
  !insertmacro FRESH
  !insertmacro NSIS_HOOK_PREINSTALL
  !insertmacro NSIS_HOOK_POSTINSTALL
  ReadRegStr $1 HKCU "${MENU}\.png\shell\InkvecStudio\command" ""
  StrCpy $R0 "yes"
  ${IfThen} $1 != "" ${|} StrCpy $R0 "a context menu was added: $1" ${|}
  ${IfThen} ${FileExists} "${WORK}\WindowsApps\inkvec.exe" ${|} StrCpy $R0 "a command was added" ${|}
  ${IfThen} ${FileExists} "${WORK}\uninstall-called.txt" ${|} StrCpy $R0 "an uninstaller ran" ${|}
  !insertmacro CHECK "without an old install, nothing is added or removed"

  ; ---- 4. An old .msi install, whose context menu names the program it removes ----------
  !insertmacro FRESH
  WriteRegStr HKCU "${INKVEC_SOFTWARE}\Microsoft\Windows\CurrentVersion\Uninstall\${DRY_GUID}" "DisplayName" "Inkvec Studio Lite"
  WriteRegStr HKCU "${INKVEC_SOFTWARE}\Microsoft\Windows\CurrentVersion\Uninstall\${DRY_GUID}" "UninstallString" "MsiExec.exe /X${DRY_GUID}"
  WriteRegStr HKCU "${MENU}\.png\shell\InkvecStudio\command" "" '"${WORK}\Program Files\Inkvec Studio Lite\inkvec-studio.exe" "%1"'
  !insertmacro NSIS_HOOK_PREINSTALL
  !insertmacro NSIS_HOOK_POSTINSTALL
  StrCpy $R0 "no log"
  ${If} ${FileExists} "${WORK}\uninstall-called.txt"
    FileOpen $0 "${WORK}\uninstall-called.txt" r
    FileRead $0 $1
    FileClose $0
    StrCpy $R0 "called as: $1"
    ${StrLoc} $2 $1 "/x ${DRY_GUID}" ">"
    ${StrLoc} $3 $1 "/passive" ">"
    ${If} $2 != ""
    ${AndIf} $3 != ""
      StrCpy $R0 "yes"
    ${EndIf}
  ${EndIf}
  !insertmacro CHECK "an old .msi is removed by its product code (msiexec /x, /passive)"
  ReadRegStr $1 HKCU "${INKVEC_SOFTWARE}\Microsoft\Windows\CurrentVersion\Uninstall\${DRY_GUID}" "DisplayName"
  StrCpy $R0 "yes"
  ${IfThen} $1 != "" ${|} StrCpy $R0 "the msi entry is still there" ${|}
  !insertmacro CHECK "the old msi Apps entry is gone"
  ReadRegStr $1 HKCU "${MENU}\.png\shell\InkvecStudio\command" ""
  StrCpy $R0 "png: $1"
  ${IfThen} $1 == '"$INSTDIR\inkvec-studio.exe" "%1"' ${|} StrCpy $R0 "yes" ${|}
  !insertmacro CHECK "a context menu naming a removed program points at the new app"

  ; ---- 5. A per-machine .exe install, removed through the shell (elevation) -------------
  !insertmacro FRESH
  !insertmacro OLD_INSTALL
  !ifmacrodef INKVEC_REMOVE_OLD
    !insertmacro INKVEC_REMOVE_OLD HKCU ELEVATED
  !endif
  StrCpy $R0 "yes"
  ${IfThen} ${FileExists} "${OLD}\*.*" ${|} StrCpy $R0 "the old folder is still there" ${|}
  ReadRegStr $1 HKCU "${OLDKEY}" "UninstallString"
  ${IfThen} $1 != "" ${|} StrCpy $R0 "entry still there: $1" ${|}
  !insertmacro CHECK "the elevated path runs the old uninstaller and removes the install"

  ; ---- 3. An Apps entry whose folder is gone -------------------------------------------
  !insertmacro FRESH
  !insertmacro OLD_INSTALL
  RMDir /r "${OLD}"
  !insertmacro NSIS_HOOK_PREINSTALL
  !insertmacro NSIS_HOOK_POSTINSTALL
  ReadRegStr $1 HKCU "${OLDKEY}" "UninstallString"
  StrCpy $R0 "yes"
  ${IfThen} $1 != "" ${|} StrCpy $R0 "the ghost entry is still there" ${|}
  !insertmacro CHECK "an Apps entry with no install behind it is removed"

  ; ---- 6. In-place upgrade of Inkvec Studio ---------------------------------------------
  !insertmacro FRESH
  WriteRegStr HKCU "${MENU}\.png\shell\InkvecStudio\command" "" '"${OLD}\inkvec-studio.exe" "%1"'
  WriteRegStr HKCU "${MENU}\.tif\shell\InkvecStudio\command" "" '"${OLD}\inkvec-studio.exe" "%1"'
  !insertmacro FILE "${WORK}\WindowsApps\inkvec.exe" "old command"

  ; The installer marks the upgrade in progress and notes integrations at startup:
  WriteRegDWORD HKCU "${INKVEC_PREFS_KEY}" "UpgradeInProgress" 1
  !insertmacro INKVEC_NOTE_INTEGRATIONS

  ; Tauri's reinstall page runs the uninstaller, which executes PREUNINSTALL:
  !insertmacro NSIS_HOOK_PREUNINSTALL

  ; Then the installer runs its install hooks:
  !insertmacro NSIS_HOOK_PREINSTALL
  !insertmacro NSIS_HOOK_POSTINSTALL

  ReadRegStr $1 HKCU "${MENU}\.png\shell\InkvecStudio\command" ""
  ReadRegStr $2 HKCU "${MENU}\.jpeg\shell\InkvecStudio\command" ""
  StrCpy $R0 "png: $1 / jpeg: $2"
  ${If} $1 == '"$INSTDIR\inkvec-studio.exe" "%1"'
  ${AndIf} $2 == $1
    StrCpy $R0 "yes"
  ${EndIf}
  !insertmacro CHECK "in-place upgrade keeps context menu, pointing at new app"

  StrCpy $R0 "missing"
  ${If} ${FileExists} "${WORK}\WindowsApps\inkvec.exe"
    FileOpen $0 "${WORK}\WindowsApps\inkvec.exe" r
    FileRead $0 $1
    FileClose $0
    StrCpy $R0 "holds: $1"
    ${IfThen} $1 == "new command" ${|} StrCpy $R0 "yes" ${|}
  ${EndIf}
  !insertmacro CHECK "in-place upgrade keeps inkvec command, updated to new copy"

  ClearErrors
  ReadRegDWORD $1 HKCU "${INKVEC_PREFS_KEY}" "UpgradeInProgress"
  StrCpy $R0 "yes"
  ${IfNot} ${Errors}
    StrCpy $R0 "marker still present: $1"
  ${EndIf}
  !insertmacro CHECK "in-place upgrade cleans up upgrade markers"

  ; ---- 7. In-place upgrade where an older uninstaller wiped integrations ----------------
  !insertmacro FRESH
  WriteRegStr HKCU "${MENU}\.png\shell\InkvecStudio\command" "" '"${OLD}\inkvec-studio.exe" "%1"'
  !insertmacro FILE "${WORK}\WindowsApps\inkvec.exe" "old command"

  ; Installer marks upgrade and stashes state:
  WriteRegDWORD HKCU "${INKVEC_PREFS_KEY}" "UpgradeInProgress" 1
  !insertmacro INKVEC_NOTE_INTEGRATIONS

  ; Older uninstaller wipes integrations unconditionally:
  DeleteRegKey HKCU "${MENU}\.png\shell\InkvecStudio"
  Delete "${WORK}\WindowsApps\inkvec.exe"

  ; Simulate clean installer process reading from registry stash:
  StrCpy $InkvecHadMenu 0
  StrCpy $InkvecHadCli 0

  !insertmacro NSIS_HOOK_PREINSTALL
  !insertmacro NSIS_HOOK_POSTINSTALL

  ReadRegStr $1 HKCU "${MENU}\.png\shell\InkvecStudio\command" ""
  StrCpy $R0 "png: $1"
  ${IfThen} $1 == '"$INSTDIR\inkvec-studio.exe" "%1"' ${|} StrCpy $R0 "yes" ${|}
  !insertmacro CHECK "upgrade restores context menu even if old uninstaller wiped it"

  StrCpy $R0 "missing"
  ${If} ${FileExists} "${WORK}\WindowsApps\inkvec.exe"
    FileOpen $0 "${WORK}\WindowsApps\inkvec.exe" r
    FileRead $0 $1
    FileClose $0
    StrCpy $R0 "holds: $1"
    ${IfThen} $1 == "new command" ${|} StrCpy $R0 "yes" ${|}
  ${EndIf}
  !insertmacro CHECK "upgrade restores inkvec command even if old uninstaller wiped it"

  ; ---- 8. True uninstall removes all shell integrations ---------------------------------
  !insertmacro FRESH
  WriteRegStr HKCU "${MENU}\.png\shell\InkvecStudio\command" "" '"$INSTDIR\inkvec-studio.exe" "%1"'
  !insertmacro FILE "${WORK}\WindowsApps\inkvec.exe" "command"

  ; True uninstall: no UpgradeInProgress marker:
  !insertmacro NSIS_HOOK_PREUNINSTALL

  ReadRegStr $1 HKCU "${MENU}\.png\shell\InkvecStudio\command" ""
  StrCpy $R0 "yes"
  ${IfThen} $1 != "" ${|} StrCpy $R0 "context menu still present: $1" ${|}
  !insertmacro CHECK "true uninstall removes context menu"

  StrCpy $R0 "yes"
  ${IfThen} ${FileExists} "${WORK}\WindowsApps\inkvec.exe" ${|} StrCpy $R0 "command still present" ${|}
  !insertmacro CHECK "true uninstall removes inkvec command"

  ClearErrors
  ReadRegDWORD $1 HKCU "${INKVEC_PREFS_KEY}" "UpgradeHadMenu"
  StrCpy $R0 "yes"
  ${IfNot} ${Errors}
    StrCpy $R0 "stashed value still present: $1"
  ${EndIf}
  !insertmacro CHECK "true uninstall leaves no stashed integration state"

  ; ---- 9. In-place upgrade with integrations disabled: nothing is added -----------------
  !insertmacro FRESH
  WriteRegDWORD HKCU "${INKVEC_PREFS_KEY}" "UpgradeInProgress" 1
  !insertmacro INKVEC_NOTE_INTEGRATIONS
  !insertmacro NSIS_HOOK_PREUNINSTALL
  !insertmacro NSIS_HOOK_PREINSTALL
  !insertmacro NSIS_HOOK_POSTINSTALL

  ReadRegStr $1 HKCU "${MENU}\.png\shell\InkvecStudio\command" ""
  StrCpy $R0 "yes"
  ${IfThen} $1 != "" ${|} StrCpy $R0 "context menu was added: $1" ${|}
  ${IfThen} ${FileExists} "${WORK}\WindowsApps\inkvec.exe" ${|} StrCpy $R0 "command was added" ${|}
  !insertmacro CHECK "upgrade with integrations off adds nothing"

  FileClose $9
  DeleteRegKey HKCU "${INKVEC_SOFTWARE}"
  RMDir /r "${WORK}"
SectionEnd
