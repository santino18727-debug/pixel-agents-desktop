# Pixel Agents Desktop

Standalone desktop viewer for [Claude Code](https://claude.ai/code) sessions — built on top of [pablodelucca/pixel-agents](https://github.com/pablodelucca/pixel-agents).

Watch your Claude Code agents animate as pixel-art characters in a virtual office, without needing VS Code.

## Status

Phase 0 (scaffold) — window opens, Rust backend compiles, file watcher ready.

## Requirements

- Windows 10/11 with WebView2 (pre-installed on Windows 11)
- Claude Code sessions at `%USERPROFILE%\.claude\projects\`

## Install

Download the latest `.msi` from [Releases](https://github.com/santino18727-debug/pixel-agents-desktop/releases) and run it.

## Development

### Prerequisites

```
winget install Rustlang.Rustup
node >= 22
```

### Run in dev mode

```sh
# Install dependencies
npm install
cd webview-ui && npm install && cd ..

# Start dev server + Tauri window
npm run dev
```

### Build

```sh
npm run build
# MSI output: src-tauri/target/release/bundle/msi/
```

### Tests

```sh
# Rust unit tests
cd src-tauri && cargo test

# Clippy (zero warnings policy)
cargo clippy -- -D warnings
```

## Architecture

```
src-tauri/src/
  main.rs              — entry point
  lib.rs               — Tauri app setup, command registration
  error.rs             — AppError enum
  jsonl_parser.rs      — JSONL line parser, AgentEvent enum
  session_registry.rs  — Session discovery via walkdir
  file_watcher.rs      — notify-debouncer-full filesystem watcher
  settings.rs          — Persisted settings via tauri-plugin-store

webview-ui/            — React 19 + TypeScript + Vite (from upstream)
  src/vscode-shim.ts   — Tauri IPC bridge (replaces VS Code postMessage)

shared/                — Shared assets/utilities (from upstream)
```

## Relation to upstream

This is a contribution toward [pablodelucca/pixel-agents](https://github.com/pablodelucca/pixel-agents).
`webview-ui/` and `shared/` are copied from upstream with minimal modifications (only `vscode-shim.ts` added and `main.tsx` updated with one import line).

## License

MIT — see upstream for original copyright.
