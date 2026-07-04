!macro NSIS_HOOK_PREINSTALL
  StrCpy $INSTDIR "$LOCALAPPDATA\Programs\${PRODUCTNAME}"
  ; www ships as a bundled resource of content-hash-named SvelteKit chunks, so
  ; every version's filenames differ. The installer overlays rather than mirrors,
  ; so without this each update leaves the previous version's chunks behind and
  ; www grows without bound. Wipe only the www subdir (the exes and uninstaller
  ; are untouched); the installer re-extracts a clean www immediately after. This
  ; also cleanses the accumulation on existing installs on their next update.
  RMDir /r "$INSTDIR\www"
  SetOutPath $INSTDIR
!macroend
