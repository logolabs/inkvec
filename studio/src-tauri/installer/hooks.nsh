; Inkvec Studio — NSIS installer hooks.
;
; Tauri's own NSIS template does the install, the upgrade and the uninstall; this file is
; the documented extension point (`bundle.windows.nsis.installerHooks`) and adds only what
; the template cannot know about. The template includes it after its own headers (LogicLib,
; FileFunc, StrFunc with ${StrCase} and ${StrLoc} declared) and before its !defines, so the
; macros below may use ${PRODUCTNAME} and ${MAINBINARYNAME}: they expand where inserted.
;
; What it does NOT do, deliberately: add a desktop shortcut or a context-menu entry. The
; design's options page needs a full template override, which would mean maintaining a
; copy of Tauri's installer and shipping it untested. Both integrations are offered — and
; are reversible — in the app's own Settings screen instead, which is a better place for
; them anyway: somebody who declines a checkbox during an install rarely finds it again.
;
; What it does do:
;
; * Replace the app's old name. Up to 0.1.7 the desktop app was "Inkvec Studio Lite", and
;   Tauri keys the install folder and the Apps entry on the product name, so the renamed
;   app would install beside the old one instead of over it. Before installing, an old
;   install is found by its Apps entry and removed by its own uninstaller: the .exe
;   installer's per-user entry silently; a per-machine one (the .msi, or an .exe built per
;   machine) with the administrator's consent, which Windows asks for. The preferences are
;   not touched: they live in %APPDATA%\inkvec-studio, which no uninstaller removes, and a
;   silent uninstall never ticks "delete app data". If the old app is open, the user is
;   asked before it is closed; declining, or a removal that fails, installs beside it as
;   before.
; * Keep the two Settings integrations across that. The old .exe's uninstaller removes
;   them (its hook is this file's PREUNINSTALL) and the .msi leaves the context menu
;   pointing at a program that is gone; after the install, whichever was on points at the
;   new app. Nothing is added that the user had not turned on.
; * Clean up after those integrations on uninstall. An uninstall that leaves registry keys
;   and a stray `inkvec.exe` on the PATH is a real defect, and the template cannot remove
;   what it did not create.
;
; `python studio/tools/check_installer_hooks.py` compiles this file into a harness and
; dry-runs it against a staged old install, in a private registry key and folder.

!include LogicLib.nsh
!include FileFunc.nsh

; The name the desktop app had, and so the Apps entry an old install left.
!define INKVEC_OLD_PRODUCT "Inkvec Studio Lite"
; The registry's Software key, the hive per-machine entries are in, msiexec, and the folder
; the `inkvec` command goes in. Only the dry test points them anywhere else, so that it
; never touches the real Apps entries, context menu or PATH of the machine it runs on.
!ifndef INKVEC_SOFTWARE
  !define INKVEC_SOFTWARE "Software"
!endif
!ifndef INKVEC_MACHINE
  !define INKVEC_MACHINE HKLM
!endif
!ifndef INKVEC_MSIEXEC
  !define INKVEC_MSIEXEC "$SYSDIR\msiexec.exe"
!endif
!ifndef INKVEC_CLI_DIR
  !define INKVEC_CLI_DIR "$LOCALAPPDATA\Microsoft\WindowsApps"
!endif
!define INKVEC_UNINSTALL "${INKVEC_SOFTWARE}\Microsoft\Windows\CurrentVersion\Uninstall"
!define INKVEC_OLD_UNINSTKEY "${INKVEC_UNINSTALL}\${INKVEC_OLD_PRODUCT}"
; The current product name, defaulted if this file is included before the template defines it.
!ifdef PRODUCTNAME
  !define INKVEC_PRODUCT "${PRODUCTNAME}"
!else
  !define INKVEC_PRODUCT "Inkvec Studio"
!endif
!define INKVEC_PREFS_KEY "${INKVEC_SOFTWARE}\LogoLabs\${INKVEC_PRODUCT}"

; Where Settings puts the two integrations; keep in step with `src/integration.rs`.
!define INKVEC_CLASSES "${INKVEC_SOFTWARE}\Classes\SystemFileAssociations"
!define INKVEC_VERB "shell\InkvecStudio"
!define INKVEC_CLI "${INKVEC_CLI_DIR}\inkvec.exe"

Var InkvecOldUninstaller
Var InkvecOldDir
Var InkvecOldBinary
Var InkvecHadMenu
Var InkvecHadCli

; Whether the old app may be removed now: sets $1 to "go", or "skip" when it is open and the
; user would rather keep it. Its own uninstaller would close it without asking when run
; silently, so the question is asked here, as the template asks it for this app.
!macro INKVEC_MAY_CLOSE BINARY
  StrCpy $1 "go"
  nsis_tauri_utils::FindProcessCurrentUser "${BINARY}"
  Pop $0
  ${If} $0 = 0
    MessageBox MB_OKCANCEL|MB_ICONINFORMATION "${INKVEC_OLD_PRODUCT} is open. It has to close so ${PRODUCTNAME} can replace it; your settings are kept.$\n$\nClose it now? Cancel installs ${PRODUCTNAME} beside it instead." /SD IDOK IDOK +2
    StrCpy $1 "skip"
    ${If} $1 == "go"
      nsis_tauri_utils::KillProcessCurrentUser "${BINARY}"
      Pop $0
      Sleep 500
    ${EndIf}
  ${EndIf}
!macroend

; What an old uninstaller is about to take away, so POSTINSTALL can put it back.
; Also stashes to the app's registry key so the state survives across uninstaller execution.
!macro INKVEC_NOTE_INTEGRATIONS
  ReadRegStr $0 HKCU "${INKVEC_CLASSES}\.png\${INKVEC_VERB}\command" ""
  ${If} $0 != ""
    StrCpy $InkvecHadMenu 1
    WriteRegDWORD HKCU "${INKVEC_PREFS_KEY}" "UpgradeHadMenu" 1
  ${EndIf}
  ${If} ${FileExists} "${INKVEC_CLI}"
    StrCpy $InkvecHadCli 1
    WriteRegDWORD HKCU "${INKVEC_PREFS_KEY}" "UpgradeHadCli" 1
  ${EndIf}
!macroend

; In a GUI or passive installer, note integrations before Tauri's reinstall page can
; run the previous version's uninstaller, and mark an upgrade in progress.
!ifdef MUI_INCLUDED
  !define MUI_CUSTOMFUNCTION_GUIINIT InkvecGuiInit
  !define MUI_CUSTOMFUNCTION_ABORT InkvecGuiAbort

  Function InkvecGuiInit
    Push $0
    !insertmacro INKVEC_NOTE_INTEGRATIONS
    WriteRegDWORD HKCU "${INKVEC_PREFS_KEY}" "UpgradeInProgress" 1
    Pop $0
  FunctionEnd

  Function InkvecGuiAbort
    DeleteRegValue HKCU "${INKVEC_PREFS_KEY}" "UpgradeInProgress"
    DeleteRegValue HKCU "${INKVEC_PREFS_KEY}" "UpgradeHadMenu"
    DeleteRegValue HKCU "${INKVEC_PREFS_KEY}" "UpgradeHadCli"
  FunctionEnd
!endif


; Remove an old .exe install registered under ROOT (HKCU, or the machine hive), if there is
; one. RUN is WAIT for a per-user install (no elevation, so it runs silently) and ELEVATED
; for a per-machine one (this installer runs as the user; Windows asks for consent).
!macro INKVEC_REMOVE_OLD ROOT RUN
  StrCpy $InkvecOldDir ""
  StrCpy $InkvecOldBinary ""
  ReadRegStr $InkvecOldUninstaller ${ROOT} "${INKVEC_OLD_UNINSTKEY}" "UninstallString"
  ${If} $InkvecOldUninstaller != ""
    ; Written by Tauri as "C:\...\uninstall.exe", quotes included.
    StrCpy $0 $InkvecOldUninstaller 1
    ${If} $0 == '"'
      StrCpy $InkvecOldUninstaller $InkvecOldUninstaller "" 1
      StrCpy $InkvecOldUninstaller $InkvecOldUninstaller -1
    ${EndIf}
    ${GetParent} $InkvecOldUninstaller $InkvecOldDir
    ReadRegStr $InkvecOldBinary ${ROOT} "${INKVEC_OLD_UNINSTKEY}" "MainBinaryName"
    ${IfThen} $InkvecOldBinary == "" ${|} StrCpy $InkvecOldBinary "${MAINBINARYNAME}.exe" ${|}

    ${If} ${FileExists} "$InkvecOldUninstaller"
      !insertmacro INKVEC_MAY_CLOSE "$InkvecOldBinary"
      ${If} $1 == "go"
        DetailPrint "Removing ${INKVEC_OLD_PRODUCT}, which ${PRODUCTNAME} replaces"
        !insertmacro INKVEC_NOTE_INTEGRATIONS
        ; `_?=` makes it uninstall in place and wait, rather than copy itself to %TEMP% and
        ; return at once; it also leaves uninstall.exe itself, removed below.
        ClearErrors
        !insertmacro INKVEC_RUN_${RUN} "$InkvecOldUninstaller" "/S _?=$InkvecOldDir"
        ${If} ${Errors}
          DetailPrint "${INKVEC_OLD_PRODUCT} could not be removed; it stays beside ${PRODUCTNAME}"
        ${Else}
          Delete "$InkvecOldUninstaller"
          RMDir "$InkvecOldDir"
        ${EndIf}
      ${EndIf}
    ${Else}
      ; The folder is gone but the Apps entry is not: a ghost that can only fail. Only the
      ; user's own entry can be removed without elevation; a machine one stays for an admin.
      DeleteRegKey ${ROOT} "${INKVEC_OLD_UNINSTKEY}"
    ${EndIf}
  ${EndIf}

  ; The install location and language Tauri keeps for the old name (under both publisher
  ; names it has shipped with), which its uninstaller leaves unless app data is deleted.
  ${If} $InkvecOldDir == ""
  ${OrIfNot} ${FileExists} "$InkvecOldDir\$InkvecOldBinary"
    DeleteRegKey ${ROOT} "${INKVEC_SOFTWARE}\LogoLabs\${INKVEC_OLD_PRODUCT}"
    DeleteRegKey /ifempty ${ROOT} "${INKVEC_SOFTWARE}\LogoLabs"
    DeleteRegKey ${ROOT} "${INKVEC_SOFTWARE}\LogoLabs SRL\${INKVEC_OLD_PRODUCT}"
    DeleteRegKey /ifempty ${ROOT} "${INKVEC_SOFTWARE}\LogoLabs SRL"
  ${EndIf}
!macroend

; Remove an old .msi install. Its Apps entry is keyed by product code, so it is found by
; name, the way the template finds its own .msi installs; msiexec asks for the consent a
; per-machine uninstall needs and shows only a progress bar.
!macro INKVEC_REMOVE_OLD_MSI
  StrCpy $2 0
  ${Do}
    EnumRegKey $3 ${INKVEC_MACHINE} "${INKVEC_UNINSTALL}" $2
    ${IfThen} $3 == "" ${|} ${ExitDo} ${|}
    IntOp $2 $2 + 1
    ReadRegStr $4 ${INKVEC_MACHINE} "${INKVEC_UNINSTALL}\$3" "DisplayName"
    ${If} $4 == "${INKVEC_OLD_PRODUCT}"
      ReadRegStr $4 ${INKVEC_MACHINE} "${INKVEC_UNINSTALL}\$3" "UninstallString"
      ${StrCase} $5 $4 "L"
      ${StrLoc} $6 $5 "msiexec" ">"
      ${If} $6 != ""
        !insertmacro INKVEC_MAY_CLOSE "${MAINBINARYNAME}.exe"
        ${If} $1 == "go"
          DetailPrint "Removing ${INKVEC_OLD_PRODUCT} (.msi), which ${PRODUCTNAME} replaces"
          ExecWait '"${INKVEC_MSIEXEC}" /x $3 /passive /norestart' $0
          ; Its entry is gone now, so the next one has moved up into this index.
          ${If} $0 = 0
          ${OrIf} $0 = 3010
            IntOp $2 $2 - 1
          ${Else}
            DetailPrint "${INKVEC_OLD_PRODUCT} (.msi) could not be removed ($0); it stays beside ${PRODUCTNAME}"
          ${EndIf}
        ${EndIf}
      ${EndIf}
    ${EndIf}
  ${Loop}
!macroend

!macro INKVEC_RUN_WAIT EXE ARGS
  ExecWait '"${EXE}" ${ARGS}' $0
  ${IfThen} $0 <> 0 ${|} SetErrors ${|}
!macroend

!macro INKVEC_RUN_ELEVATED EXE ARGS
  ExecShellWait "" "${EXE}" "${ARGS}"
!macroend

; The "Vectorize with Inkvec" verb for one extension, as `src/integration.rs` writes it.
!macro INKVEC_WRITE_VERB EXT
  WriteRegStr HKCU "${INKVEC_CLASSES}\${EXT}\${INKVEC_VERB}" "" "Vectorize with Inkvec"
  WriteRegStr HKCU "${INKVEC_CLASSES}\${EXT}\${INKVEC_VERB}" "Icon" "$INSTDIR\${MAINBINARYNAME}.exe,0"
  WriteRegStr HKCU "${INKVEC_CLASSES}\${EXT}\${INKVEC_VERB}\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; The registers are the template's too; hand them back as they were.
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  StrCpy $InkvecHadMenu 0
  StrCpy $InkvecHadCli 0
  ; In a silent or direct install, note integrations before any old uninstallers run.
  !insertmacro INKVEC_NOTE_INTEGRATIONS
  !insertmacro INKVEC_REMOVE_OLD HKCU WAIT
  !insertmacro INKVEC_REMOVE_OLD ${INKVEC_MACHINE} ELEVATED
  !insertmacro INKVEC_REMOVE_OLD_MSI
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Push $0
  Push $1
  Push $2
  ; Read any stashed upgrade state from registry (written by GUI init or PREUNINSTALL)
  ReadRegDWORD $0 HKCU "${INKVEC_PREFS_KEY}" "UpgradeHadMenu"
  ${IfThen} $0 = 1 ${|} StrCpy $InkvecHadMenu 1 ${|}
  DeleteRegValue HKCU "${INKVEC_PREFS_KEY}" "UpgradeHadMenu"

  ReadRegDWORD $0 HKCU "${INKVEC_PREFS_KEY}" "UpgradeHadCli"
  ${IfThen} $0 = 1 ${|} StrCpy $InkvecHadCli 1 ${|}
  DeleteRegValue HKCU "${INKVEC_PREFS_KEY}" "UpgradeHadCli"

  DeleteRegValue HKCU "${INKVEC_PREFS_KEY}" "UpgradeInProgress"

  ; The context menu: back if an old uninstaller removed it, and re-pointed if it still
  ; names a program that is no longer there (an old .msi's). "C:\...\app.exe" "%1".
  ReadRegStr $0 HKCU "${INKVEC_CLASSES}\.png\${INKVEC_VERB}\command" ""
  StrCpy $1 ""
  ${If} $0 != ""
    StrCpy $1 $0 "" 1
    ${StrLoc} $2 $1 '"' ">"
    ${IfThen} $2 != "" ${|} StrCpy $1 $1 $2 ${|}
  ${EndIf}
  StrCpy $2 "keep"
  ${If} $0 == ""
    ${IfThen} $InkvecHadMenu = 1 ${|} StrCpy $2 "write" ${|}
  ${ElseIfNot} ${FileExists} "$1"
    StrCpy $2 "write"
  ${ElseIf} $InkvecHadMenu = 1
    StrCpy $2 "write"
  ${EndIf}
  ${If} $2 == "write"
    !insertmacro INKVEC_WRITE_VERB ".png"
    !insertmacro INKVEC_WRITE_VERB ".jpg"
    !insertmacro INKVEC_WRITE_VERB ".jpeg"
    !insertmacro INKVEC_WRITE_VERB ".webp"
    !insertmacro INKVEC_WRITE_VERB ".bmp"
    !insertmacro INKVEC_WRITE_VERB ".tif"
  ${EndIf}
  ; The command: a copy of this version's, if an old uninstaller took the one there was,
  ; or if the user had it and it needs updating to this version's binary.
  ${If} $InkvecHadCli = 1
  ${AndIf} ${FileExists} "$INSTDIR\inkvec.exe"
    CreateDirectory "${INKVEC_CLI_DIR}"
    CopyFiles /SILENT "$INSTDIR\inkvec.exe" "${INKVEC_CLI}"
  ${EndIf}
  Pop $2
  Pop $1
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Push $0
  ; Check if an installer marked an upgrade in progress.
  ReadRegDWORD $0 HKCU "${INKVEC_PREFS_KEY}" "UpgradeInProgress"
  ${If} $0 = 1
    ; In an in-place upgrade, preserve the user's integrations. Stash their state
    ; so POSTINSTALL knows they were enabled even if an uninstaller cleans files.
    !insertmacro INKVEC_NOTE_INTEGRATIONS
  ${Else}
    ; The per-user "Vectorize with Inkvec" verb, if Settings ever added it. Written under
    ; SystemFileAssociations so it offers an action on these types without claiming to be
    ; their default handler; keep this list in step with `src/integration.rs`.
    DeleteRegKey HKCU "${INKVEC_CLASSES}\.png\${INKVEC_VERB}"
    DeleteRegKey HKCU "${INKVEC_CLASSES}\.jpg\${INKVEC_VERB}"
    DeleteRegKey HKCU "${INKVEC_CLASSES}\.jpeg\${INKVEC_VERB}"
    DeleteRegKey HKCU "${INKVEC_CLASSES}\.webp\${INKVEC_VERB}"
    DeleteRegKey HKCU "${INKVEC_CLASSES}\.bmp\${INKVEC_VERB}"
    DeleteRegKey HKCU "${INKVEC_CLASSES}\.tif\${INKVEC_VERB}"

    ; The `inkvec` command, if Settings ever put it on the PATH.
    Delete "${INKVEC_CLI}"

    DeleteRegValue HKCU "${INKVEC_PREFS_KEY}" "UpgradeHadMenu"
    DeleteRegValue HKCU "${INKVEC_PREFS_KEY}" "UpgradeHadCli"
    DeleteRegValue HKCU "${INKVEC_PREFS_KEY}" "UpgradeInProgress"
  ${EndIf}
  Pop $0
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
