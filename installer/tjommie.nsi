; Tjommie's installer (Phase 3, T8.2): the hand-written NSIS script that installs the native Windows app where R1's electron-builder
; installer put the Electron app, and over it. It is written to what T8.1 recorded from a real R1 0.5.1 install
; (installer/r1-install-facts.json), not to the plan's text: scripts/installer.test.ts checks every key, value name, path and
; AppUserModelID below against that file.
;
; It is built by installer/build-installer.ps1, which gives the defines below (one set per identity in installer/identities.json)
; and the folder holding the payload. Never compile this by hand for a release: GitHub Actions does that.
;
; What it does
;   Per-user and one-click (no wizard pages); silent with /S. It never asks for elevation and never writes outside the user's
;   profile. It installs to %LOCALAPPDATA%\Programs\<folder> and nowhere else (for the identities with root "temp", to the folder
;   given with /D=, which must be two levels under %TEMP% and named for the product, as R1's update test installs).
;   It accepts electron-updater's arguments: --updated says this is an update, --force-run starts the app when the install is
;   silent. After a silent install without --force-run it starts nothing (R1 doesn't either).
;
; Over an existing install (R1's, or an earlier one of this installer's)
;   0. Before it extracts or stops anything (in .onInit) it reads HKCU\Software\<GUID>\InstallLocation. If that is set to any folder
;      but this identity's own it stops with exit 7 and touches nothing, and never "corrects" the value. R1's uninstaller, which
;      step 3 runs, deletes the folder that value names whenever it is set, and ignores the folder it is told; a stale, wrong or
;      planted value would have it delete somewhere else. This is the mechanism that destroyed the owner's install on 2026-10-25.
;   1. It closes the processes whose executable is inside the install folder, and their children (stop-install-processes.ps1),
;      by path and never by name. A Tjommie running from another folder is left alone, and so is a program with a window of its
;      own that the app opened (a browser).
;   2. It removes the directory links (junctions) inside the install folder as links (unlink-reparse-points.ps1), because the
;      deletions that follow, R1's and its own, would otherwise follow them into whatever they point at.
;   3. It runs the existing uninstaller from a temp copy with the arguments R1's own upgrade uses (recorded in the facts):
;        old-uninstaller.exe /S /KEEP_APP_DATA /currentuser --keep-shortcuts --updated _?=<install folder>
;   4. It copies the payload, writes the uninstaller and the registry keys, and (re)makes the shortcuts.
;   If any of steps 0 to 3 fails it installs nothing, so the old app and the new one never run on one data root.
;   One thing here is not this script's to promise: R1's uninstaller stops programs on its own. T8.2's review found that, when
;   PowerShell is usable, it stops every process whose path starts with the install folder's path with no trailing separator
;   (so one running from ...\Programs\tjommie-rs or ...\tjommie-updatetest too), and otherwise every Tjommie.exe of the user by
;   image name. Close the dev and update-test apps before updating production.
;
; The uninstaller deletes only the install folder, its own registry keys and its shortcuts. The data roots (%LOCALAPPDATA%\Tjommie
; and %APPDATA%\Tjommie) are never touched, and nothing here names them. It refuses to run, and deletes nothing, unless
; HKCU\Software\<GUID>\InstallLocation is exactly the install folder this identity installs to AND is the folder the
; uninstaller is running for. That guard is what stands between this and the test uninstaller that once deleted the owner's
; install: deletion always targets the folder the uninstaller runs from, never a registry value. If the value is ever missing
; (a cleaner, a hand edit), run this installer again: it rewrites it, and the uninstaller then works.
; It carries its own helper scripts and runs them from its own temp folder, so nothing in the install folder is needed to remove it.
;
; Exit codes (silent or not)
;   0 done.  3 something is still running from the install folder.  4 the old uninstaller failed.  5 the uninstaller refused
;   (InstallLocation is not this identity's install folder).  6 the install folder is not allowed for this identity.
;   7 this identity is installed somewhere else.  8 a helper could not run (PowerShell missing or blocked by policy).
;   9 the install folder is a link, or a link in it could not be removed.

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user

!include "LogicLib.nsh"
!include "FileFunc.nsh"

; ---- what the build gives -------------------------------------------------------------------------------------------------
!macro RequireDefine NAME
  !ifndef ${NAME}
    !error "${NAME} is not defined: build with installer/build-installer.ps1."
  !endif
!macroend
!insertmacro RequireDefine APP_ID        ; com.tsnyders.tjommie: the AppUserModelID, and the name of the Run value
!insertmacro RequireDefine APP_GUID      ; 89b4e4b4-...: names the Uninstall key and the Software key
!insertmacro RequireDefine PRODUCT_NAME  ; Tjommie: shortcuts, the uninstaller's name, DisplayName
!insertmacro RequireDefine EXE_NAME      ; Tjommie.exe
!insertmacro RequireDefine FOLDER_NAME   ; tjommie: the folder under %LOCALAPPDATA%\Programs
!insertmacro RequireDefine ROOT_POLICY   ; programs | temp
!insertmacro RequireDefine VERSION       ; 0.6.0
!insertmacro RequireDefine PUBLISHER     ; TSnyders
!insertmacro RequireDefine COMMENTS      ; AI agents on your own PC
!insertmacro RequireDefine PAYLOAD_DIR   ; the folder whose contents become the install folder
!insertmacro RequireDefine OUT_FILE      ; the installer to write

; Owner decision pending (2026-11-14): R1's uninstaller leaves the "start at login" value behind, pointing at a deleted exe. By
; default this one removes it, but only when its data points into the install folder being removed, and never during an
; update (--updated). Comment this line out to leave the value as R1 does.
!define REMOVE_RUN_VALUE_ON_UNINSTALL

!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_GUID}"
!define INSTALL_KEY "Software\${APP_GUID}"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define UNINSTALLER "Uninstall ${PRODUCT_NAME}.exe"
!define POWERSHELL '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File'

!define EXIT_RUNNING 3
!define EXIT_OLD_UNINSTALLER 4
!define EXIT_REFUSED 5
!define EXIT_BAD_FOLDER 6
!define EXIT_ELSEWHERE 7
!define EXIT_HELPER 8
!define EXIT_LINK 9

Name "${PRODUCT_NAME}"
OutFile "${OUT_FILE}"
!if "${ROOT_POLICY}" == "programs"
  InstallDir "$LOCALAPPDATA\Programs\${FOLDER_NAME}"
!else
  InstallDir "$TEMP\${FOLDER_NAME}\${PRODUCT_NAME}"
!endif
BrandingText " "
AutoCloseWindow true
ShowInstDetails nevershow
ShowUninstDetails nevershow

; The installer's own version resource. The Rust updater reads AppId from it and refuses an installer that isn't its own app's.
VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey /LANG=1033 "CompanyName" "${PUBLISHER}"
VIAddVersionKey /LANG=1033 "FileDescription" "${PRODUCT_NAME} installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Copyright (C) 2026 ${PUBLISHER}"
VIAddVersionKey /LANG=1033 "AppId" "${APP_ID}"

Page instfiles
UninstPage instfiles

Var /GLOBAL Updated
Var /GLOBAL ForceRun
Var /GLOBAL KeepShortcuts
Var /GLOBAL HadInstall
Var /GLOBAL MyPid

!insertmacro GetParameters
!insertmacro GetOptions
!insertmacro GetSize
!insertmacro un.GetParameters
!insertmacro un.GetOptions
!if "${ROOT_POLICY}" == "temp"
  ; Only the folder check of an identity installed under %TEMP% takes a path apart.
  !insertmacro GetParent
  !insertmacro GetFileName
  !insertmacro un.GetParent
  !insertmacro un.GetFileName
!endif

; ---- shared by the installer and the uninstaller ----------------------------------------------------------------------------
!macro Common un

; $R0 in and out: the path in its long form (an 8.3 name such as RUNNER~1 becomes the real one) without a trailing backslash,
; so two spellings of one folder compare equal. A path that does not exist keeps the spelling it has.
Function ${un}Normalize
  Push $R3
  Push $R4
  System::Call 'kernel32::GetLongPathNameW(w r10, w .r13, i ${NSIS_MAX_STRLEN}) i .r14'
  ${If} $R4 > 0
    StrCpy $R0 $R3
  ${EndIf}
  StrCpy $R3 $R0 1 -1
  ${If} $R3 == "\"
    StrCpy $R0 $R0 -1
  ${EndIf}
  Pop $R4
  Pop $R3
FunctionEnd

; $R0 and $R1 in: two paths. $R2 out: 1 if they are the same folder (case and a trailing backslash don't matter), else 0.
Function ${un}SamePath
  Push $R3
  Call ${un}Normalize
  StrCpy $R3 $R0
  StrCpy $R0 $R1
  Call ${un}Normalize
  ${If} $R3 == $R0
    StrCpy $R2 1
  ${Else}
    StrCpy $R2 0
  ${EndIf}
  Pop $R3
FunctionEnd

; $INSTDIR (already normalized) in. $R2 out: 1 if this identity may be installed to, or uninstalled from, that folder, else 0.
Function ${un}FolderAllowed
  Push $R3
  Push $R4
  !if "${ROOT_POLICY}" == "programs"
    StrCpy $R0 $INSTDIR
    StrCpy $R1 "$LOCALAPPDATA\Programs\${FOLDER_NAME}"
    Call ${un}SamePath
  !else
    ; <TEMP>\<anything>\<product>, the shape R1's update test uses
    ${${un}GetFileName} "$INSTDIR" $R3
    ${${un}GetParent} "$INSTDIR" $R4
    ${${un}GetParent} "$R4" $R4
    StrCpy $R0 $R4
    StrCpy $R1 $TEMP
    Call ${un}SamePath
    ${If} $R3 != "${PRODUCT_NAME}"
      StrCpy $R2 0
    ${EndIf}
  !endif
  Pop $R4
  Pop $R3
FunctionEnd

; Sets $Updated, $ForceRun and $KeepShortcuts from the command line. /S, /D= and _?= are NSIS's own and need no handling.
Function ${un}ReadFlags
  ${${un}GetParameters} $R0
  StrCpy $Updated 0
  StrCpy $ForceRun 0
  StrCpy $KeepShortcuts 0
  ClearErrors
  ${${un}GetOptions} $R0 "--updated" $R1
  ${IfNot} ${Errors}
    StrCpy $Updated 1
  ${EndIf}
  ClearErrors
  ${${un}GetOptions} $R0 "--force-run" $R1
  ${IfNot} ${Errors}
    StrCpy $ForceRun 1
  ${EndIf}
  ClearErrors
  ${${un}GetOptions} $R0 "--keep-shortcuts" $R1
  ${IfNot} ${Errors}
    StrCpy $KeepShortcuts 1
  ${EndIf}
  ClearErrors
FunctionEnd

; $R0 = the folder, $R1 = the helper script. $R2 out: its exit code (0: nothing runs from the folder any more), or "error" if
; PowerShell could not be started. This process's own id is passed so an uninstaller running from inside the folder survives.
; $R0 has no trailing backslash (Normalize took it off): inside quotes a final \ would escape the closing quote.
Function ${un}StopRunning
  System::Call 'kernel32::GetCurrentProcessId() i .r0'
  StrCpy $MyPid $0
  nsExec::ExecToStack '${POWERSHELL} "$R1" -Folder "$R0" -ExceptProcessId $MyPid'
  Pop $R2
  Pop $R3
  DetailPrint "$R3"
FunctionEnd

; $R0 = the folder, $R1 = the helper script (unlink-reparse-points.ps1). $R2 out: its exit code (0: no link is left in the folder), or
; "error" if PowerShell could not be started. Same quoting rule as above.
Function ${un}Unlink
  nsExec::ExecToStack '${POWERSHELL} "$R1" -Folder "$R0"'
  Pop $R2
  Pop $R3
  DetailPrint "$R3"
FunctionEnd

!macroend
!insertmacro Common ""
!insertmacro Common "un."

; ---- the installer ----------------------------------------------------------------------------------------------------------
Function .onInit
  InitPluginsDir
  Call ReadFlags
  !if "${ROOT_POLICY}" == "programs"
    ; The folder is fixed: /D= is ignored.
    StrCpy $INSTDIR "$LOCALAPPDATA\Programs\${FOLDER_NAME}"
  !endif
  StrCpy $R0 $INSTDIR
  Call Normalize
  StrCpy $INSTDIR $R0
  Call FolderAllowed
  ${If} $R2 != 1
    MessageBox MB_OK|MB_ICONSTOP "${PRODUCT_NAME} cannot be installed to $INSTDIR." /SD IDOK
    SetErrorLevel ${EXIT_BAD_FOLDER}
    Quit
  ${EndIf}
  ; An install that this identity's own key says is somewhere else is not this installer's to replace. The old uninstaller that the
  ; install section runs deletes the folder this value names (it ignores the one it is given), so a stale or planted value must
  ; stop the install here, before anything is extracted, stopped or run. The value is never "corrected": owner data may be at stake.
  ReadRegStr $R1 HKCU "${INSTALL_KEY}" "InstallLocation"
  ${If} $R1 != ""
    StrCpy $R0 $INSTDIR
    Call SamePath
    ${If} $R2 != 1
      MessageBox MB_OK|MB_ICONSTOP "${PRODUCT_NAME} is recorded as installed in $R1, not in $INSTDIR, so nothing was changed. Remove that install first, or fix the entry by hand." /SD IDOK
      SetErrorLevel ${EXIT_ELSEWHERE}
      Quit
    ${EndIf}
  ${EndIf}
FunctionEnd

; $R0 = the shortcut's path. Makes it as R1 does, then sets its AppUserModelID. An update leaves out a shortcut the user removed.
Function MakeShortcut
  ${If} $HadInstall == 1
  ${AndIfNot} ${FileExists} "$R0"
    Return
  ${EndIf}
  CreateShortCut "$R0" "$INSTDIR\${EXE_NAME}" "" "$INSTDIR\${EXE_NAME}" 0 SW_SHOWNORMAL "" "${COMMENTS}"
  nsExec::ExecToStack '${POWERSHELL} "$PLUGINSDIR\set-shortcut-aumid.ps1" -Path "$R0" -AppUserModelId "${APP_ID}"'
  Pop $R2
  Pop $R3
  ; Without the ID the app still starts and works; its notifications do not appear. The acceptance run checks for it.
  ${If} $R2 != 0
    DetailPrint "The AppUserModelID could not be set on $R0: $R3"
  ${EndIf}
FunctionEnd

Section "Install"
  SetShellVarContext current
  SetOutPath "$PLUGINSDIR"
  File "${__FILEDIR__}\scoped-processes.ps1"
  File "${__FILEDIR__}\stop-install-processes.ps1"
  File "${__FILEDIR__}\unlink-reparse-points.ps1"
  File "${__FILEDIR__}\set-shortcut-aumid.ps1"

  StrCpy $HadInstall 0
  ${If} ${FileExists} "$INSTDIR\${UNINSTALLER}"
    StrCpy $HadInstall 1
  ${EndIf}

  ${If} ${FileExists} "$INSTDIR\*.*"
    ; 1. Nothing may run from the folder: not the old app, and not its daemon, on the data root the new app is about to use.
    StrCpy $R0 "$INSTDIR"
    StrCpy $R1 "$PLUGINSDIR\stop-install-processes.ps1"
    Call StopRunning
    ${If} $R2 == 3
      MessageBox MB_OK|MB_ICONSTOP "${PRODUCT_NAME} is running and could not be closed. Close it and run the installer again." /SD IDOK
      SetErrorLevel ${EXIT_RUNNING}
      Abort
    ${ElseIf} $R2 != 0
      MessageBox MB_OK|MB_ICONSTOP "The installer could not check whether ${PRODUCT_NAME} is running (PowerShell did not run the helper: $R2). Nothing was changed." /SD IDOK
      SetErrorLevel ${EXIT_HELPER}
      Abort
    ${EndIf}

    ; 2. No link inside the folder may be left for a deletion to follow: not R1's uninstaller's, which is next, and not this one's.
    StrCpy $R0 "$INSTDIR"
    StrCpy $R1 "$PLUGINSDIR\unlink-reparse-points.ps1"
    Call Unlink
    ${If} $R2 == 4
    ${OrIf} $R2 == 5
      MessageBox MB_OK|MB_ICONSTOP "The ${PRODUCT_NAME} folder is, or holds, a link that cannot be removed safely, so nothing was changed: $R3" /SD IDOK
      SetErrorLevel ${EXIT_LINK}
      Abort
    ${ElseIf} $R2 != 0
      MessageBox MB_OK|MB_ICONSTOP "The installer could not check the ${PRODUCT_NAME} folder for links (PowerShell did not run the helper: $R2). Nothing was changed." /SD IDOK
      SetErrorLevel ${EXIT_HELPER}
      Abort
    ${EndIf}
  ${EndIf}

  ; 3. The old version removes its own files first, from a temp copy of its uninstaller, in keep-data mode, as an update does.
  ${If} $HadInstall == 1
    ClearErrors
    CopyFiles /SILENT "$INSTDIR\${UNINSTALLER}" "$PLUGINSDIR\old-uninstaller.exe"
    ${If} ${Errors}
      MessageBox MB_OK|MB_ICONSTOP "The old version of ${PRODUCT_NAME} could not be prepared for removal. Nothing was changed." /SD IDOK
      SetErrorLevel ${EXIT_OLD_UNINSTALLER}
      Abort
    ${EndIf}
    ; $R0 must not still hold something from above when the program fails to start: it starts as "not run", which is not 0.
    StrCpy $R0 "not run"
    ClearErrors
    ExecWait '"$PLUGINSDIR\old-uninstaller.exe" /S /KEEP_APP_DATA /currentuser --keep-shortcuts --updated _?=$INSTDIR' $R0
    ${If} ${Errors}
    ${OrIf} $R0 != 0
      MessageBox MB_OK|MB_ICONSTOP "The old version of ${PRODUCT_NAME} could not be removed (code $R0). Run the installer again." /SD IDOK
      SetErrorLevel ${EXIT_OLD_UNINSTALLER}
      Abort
    ${EndIf}
  ${EndIf}

  ; 4. The new files.
  SetOutPath "$INSTDIR"
  SetOverwrite on
  File /r "${PAYLOAD_DIR}\*.*"
  WriteUninstaller "$INSTDIR\${UNINSTALLER}"

  ; HKCU\Software\<GUID>: where the install is recorded, read by the next install and by the uninstaller's guard.
  WriteRegStr HKCU "${INSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${INSTALL_KEY}" "KeepShortcuts" "true"
  WriteRegStr HKCU "${INSTALL_KEY}" "ShortcutName" "${PRODUCT_NAME}"

  ; The Apps & features entry, with the values R1's has and no others.
  ${GetSize} "$INSTDIR" "/S=0K" $R0 $R1 $R2
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Comments" "${COMMENTS}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${EXE_NAME},0"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT_NAME} ${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $R0
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\${UNINSTALLER}" /currentuser /S'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\${UNINSTALLER}" /currentuser'

  ; The shortcuts: the user's Start menu and desktop, never the all-users ones.
  StrCpy $R0 "$SMPROGRAMS\${PRODUCT_NAME}.lnk"
  Call MakeShortcut
  StrCpy $R0 "$DESKTOP\${PRODUCT_NAME}.lnk"
  Call MakeShortcut
SectionEnd

; After a silent install only --force-run starts the app (electron-updater passes it when the user chose Restart to update);
; after a normal one it always starts. --updated tells the new app to start in the tray.
Function .onInstSuccess
  ${If} $ForceRun == 1
  ${OrIfNot} ${Silent}
    SetOutPath "$INSTDIR"
    ${If} $Updated == 1
      Exec '"$INSTDIR\${EXE_NAME}" --updated'
    ${Else}
      Exec '"$INSTDIR\${EXE_NAME}"'
    ${EndIf}
  ${EndIf}
FunctionEnd

; ---- the uninstaller --------------------------------------------------------------------------------------------------------
; Runs in the copy NSIS makes in %TEMP% (or in place when given _?=), with $INSTDIR already the folder it is removing.
Function un.onInit
  InitPluginsDir
  Call un.ReadFlags
  StrCpy $R0 $INSTDIR
  Call un.Normalize
  StrCpy $INSTDIR $R0
  Call un.FolderAllowed
  ${If} $R2 == 1
    ReadRegStr $R1 HKCU "${INSTALL_KEY}" "InstallLocation"
    StrCpy $R0 $INSTDIR
    Call un.SamePath
  ${EndIf}
  ${If} $R2 != 1
    MessageBox MB_OK|MB_ICONSTOP "${PRODUCT_NAME} was not removed: its install location is not where this uninstaller belongs." /SD IDOK
    SetErrorLevel ${EXIT_REFUSED}
    Quit
  ${EndIf}
  ; Started by hand (the Start menu, Apps & features), it asks first, as R1's does. An update, and anything silent, never asks.
  ${If} $Updated != 1
  ${AndIfNot} ${Silent}
    MessageBox MB_YESNO|MB_ICONQUESTION "Remove ${PRODUCT_NAME} from this computer? Your data is kept." /SD IDYES IDYES +2
    Quit
  ${EndIf}
FunctionEnd

Section "Uninstall"
  SetShellVarContext current

  ; The helpers are unpacked from the uninstaller itself into its own temp folder: nothing in the install folder, which may have been
  ; quarantined, locked or edited, is needed to remove the app, and none of it is run.
  SetOutPath "$PLUGINSDIR"
  File "${__FILEDIR__}\scoped-processes.ps1"
  File "${__FILEDIR__}\stop-install-processes.ps1"
  File "${__FILEDIR__}\unlink-reparse-points.ps1"

  ; Nothing may run from the folder while it is removed.
  StrCpy $R0 "$INSTDIR"
  StrCpy $R1 "$PLUGINSDIR\stop-install-processes.ps1"
  Call un.StopRunning
  ${If} $R2 == 3
    MessageBox MB_OK|MB_ICONSTOP "${PRODUCT_NAME} is running and could not be closed. Close it and run the uninstaller again." /SD IDOK
    SetErrorLevel ${EXIT_RUNNING}
    Abort
  ${ElseIf} $R2 != 0
    MessageBox MB_OK|MB_ICONSTOP "The uninstaller could not check whether ${PRODUCT_NAME} is running (PowerShell did not run the helper: $R2). Nothing was removed." /SD IDOK
    SetErrorLevel ${EXIT_HELPER}
    Abort
  ${EndIf}

  ; No link inside the folder may be left for the deletion below to follow. A folder that is itself a link, or a link that will not
  ; go, stops the uninstall here, before the registry or a shortcut or a file has been touched.
  StrCpy $R0 "$INSTDIR"
  StrCpy $R1 "$PLUGINSDIR\unlink-reparse-points.ps1"
  Call un.Unlink
  ${If} $R2 == 4
  ${OrIf} $R2 == 5
    MessageBox MB_OK|MB_ICONSTOP "The ${PRODUCT_NAME} folder is, or holds, a link that cannot be removed safely, so nothing was removed: $R3" /SD IDOK
    SetErrorLevel ${EXIT_LINK}
    Abort
  ${ElseIf} $R2 != 0
    MessageBox MB_OK|MB_ICONSTOP "The uninstaller could not check the ${PRODUCT_NAME} folder for links (PowerShell did not run the helper: $R2). Nothing was removed." /SD IDOK
    SetErrorLevel ${EXIT_HELPER}
    Abort
  ${EndIf}

  ; An update keeps its registry keys and shortcuts (and the start-at-login value) for the installer that follows.
  ${If} $Updated != 1
    !ifdef REMOVE_RUN_VALUE_ON_UNINSTALL
      ; The app writes "<exe>" --hidden. The value goes only if that exe is inside this folder.
      ReadRegStr $R0 HKCU "${RUN_KEY}" "${APP_ID}"
      StrLen $R1 "$INSTDIR\"
      StrCpy $R2 $R0 1
      StrCpy $R3 $R0 $R1 1
      ${If} $R2 == '"'
      ${AndIf} $R3 == "$INSTDIR\"
        DeleteRegValue HKCU "${RUN_KEY}" "${APP_ID}"
      ${EndIf}
    !endif
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    DeleteRegKey HKCU "${INSTALL_KEY}"
    ${If} $KeepShortcuts != 1
      Delete "$SMPROGRAMS\${PRODUCT_NAME}.lnk"
      Delete "$DESKTOP\${PRODUCT_NAME}.lnk"
    ${EndIf}
  ${EndIf}

  ; The install folder, which the guard in un.onInit has shown to be this identity's own. Run from a temp copy, the uninstaller
  ; removes itself with it; run in place (_?=) it cannot, and the installer that follows overwrites it.
  ; /REBOOTOK: a file that something holds open at this moment (a virus scanner) is deleted at the next restart, not skipped.
  SetOutPath "$TEMP"
  RMDir /r /REBOOTOK "$INSTDIR"
SectionEnd
