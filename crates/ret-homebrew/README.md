# Homebrew R

## Notes

- Detects Homebrew-managed R installations from common Cellar roots:
  - `/opt/homebrew/Cellar`
  - `/usr/local/Cellar`
  - `/home/linuxbrew/.linuxbrew/Cellar`
- Looks for the `r` formula and versioned `r@*` formulae.
- Resolves each installation from `<formula>/<version>/lib/R/bin/R`.
- Reports `brew` as the manager when a known Homebrew binary is available.
- Also classifies an installation as `Homebrew` when the resolved executable or `R.home()` path points into a Homebrew layout.
