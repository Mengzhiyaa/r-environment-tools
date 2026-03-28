# macOS R Framework

## Notes

- Detects R installed under `/Library/Frameworks/R.framework/Versions`.
- Treats each version directory as a candidate installation and resolves the runtime through `Resources/bin/R`.
- Reports installations as `MacFramework`.
- This locator is only enabled on macOS.
