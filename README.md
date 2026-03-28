# R environment tools

Performant R installation discovery and JSON-RPC tooling.

This workspace provides `ret`, a CLI and stdio JSON-RPC server for discovering R installations, classifying them by manager, and resolving normalized installation metadata. For the protocol surface, see [docs/JSONRPC.md](./docs/JSONRPC.md). For a minimal client, see [docs/sample.js](./docs/sample.js).

## Installation Types Supported

- Conda and Mamba-managed R installs
- Pixi
- rig
- Homebrew
- Nix
- Guix
- Spack
- Environment Modules
- macOS framework installs
- MacPorts
- Linux global installs
- Windows Registry
- R for Windows (HQ layout)
- Scoop
- Chocolatey
- PATH and other global executable locations

## Features

- Discovery of global and manager-owned R installations
- Explicit directory and executable search
- Resolution of a single `R`, `Rscript`, or installation home into normalized metadata
- JSON-RPC server and CLI both use native RET installation schema

## Workspace Docs

- [JSON-RPC reference](./docs/JSONRPC.md)
- [JSON-RPC sample client](./docs/sample.js)
- [RET crate overview](./crates/ret/README.md)
