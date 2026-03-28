# MacPorts R

## Notes

- Detects R installations installed under the MacPorts prefix.
- Recognizes:
  - `/opt/local/bin/R`
  - `/opt/local/lib/R`
  - `/opt/local/Library/Frameworks/R.framework/Versions/*/Resources`
- Reports the `port` manager when `/opt/local/bin/port` is present.
