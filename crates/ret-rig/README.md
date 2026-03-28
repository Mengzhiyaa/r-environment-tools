# Rig

## Notes

- Detects R installations managed by `rig`.
- Recognizes the common install roots used by rig:
  - `/opt/R`
  - `/Library/Frameworks/R.framework/Versions`
  - `C:\Program Files\R`
- Classifies an installation as `Rig` when the resolved `R` executable or `R.home()` path matches one of those layouts.
- Reports the `rig` manager when a configured executable or a well-known `rig` binary location is available.
