# RET

`ret` is the CLI and JSON-RPC entrypoint for the R environment discovery workspace.

For workspace-level documentation, see:

- [../../README.md](../../README.md)
- [../../docs/JSONRPC.md](../../docs/JSONRPC.md)
- [../../docs/sample.js](../../docs/sample.js)

## Commands

- `find`: discover R installations and print them as text or JSON
- `resolve`: resolve a single `R` or `Rscript` path into normalized installation metadata
- `server`: start the stdio JSON-RPC server

## Design Principles

- Prefer filesystem inspection over spawning external tools.
- Resolve version and `R.home()` only when needed.
- Report installations in a single pass once enough metadata is available.
- Keep locator behavior consistent so higher-level integrations stay familiar.

## Search Strategy

`find` combines three sources of discovery:

1. Locator-specific scans such as Conda, rig, Homebrew, macOS framework installs, and Windows Registry.
2. Global executable search in known OS search locations and `PATH`.
3. Explicit files or directories supplied on the CLI.

When a raw executable is found, RET first asks each locator to classify it. If no locator matches, RET resolves the runtime directly and reports a generic installation.
