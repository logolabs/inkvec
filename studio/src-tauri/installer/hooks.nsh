; Inkvec Studio — NSIS installer hooks.
;
; Tauri's own NSIS template does the install, the upgrade and the uninstall; this file is
; the documented extension point (`bundle.windows.nsis.installerHooks`) and adds only what
; the template cannot know about.
;
; What it does NOT do, deliberately: add a desktop shortcut or a context-menu entry. The
; design's options page needs a full template override, which would mean maintaining a
; copy of Tauri's installer and shipping it untested. Both integrations are offered — and
; are reversible — in the app's own Settings screen instead, which is a better place for
; them anyway: somebody who declines a checkbox during an install rarely finds it again.
;
; What it does do is clean up after those integrations on uninstall. An uninstall that
; leaves registry keys and a stray `inkvec.exe` on the PATH is a real defect, and the
; template cannot remove what it did not create.

!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; The per-user "Vectorize with Inkvec" verb, if Settings ever added it. Written under
  ; SystemFileAssociations so it offers an action on these types without claiming to be
  ; their default handler; keep this list in step with `src/integration.rs`.
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.png\shell\InkvecStudio"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.jpg\shell\InkvecStudio"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.jpeg\shell\InkvecStudio"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.webp\shell\InkvecStudio"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.bmp\shell\InkvecStudio"
  DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\.tif\shell\InkvecStudio"

  ; The `inkvec` command, if Settings ever put it on the PATH.
  Delete "$LOCALAPPDATA\Microsoft\WindowsApps\inkvec.exe"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
