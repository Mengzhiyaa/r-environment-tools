# Nix R

## Notes

- Detects R installations surfaced through common Nix profiles.
- Recognizes resolved paths under:
  - `/nix/store`
  - `/nix/var/nix/profiles`
  - `/run/current-system/sw`
  - `~/.nix-profile`
  - `/etc/profiles/per-user/<user>`
- Reports the `nix` manager when a known `nix` or `nix-env` executable is available.
