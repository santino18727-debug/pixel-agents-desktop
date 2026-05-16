# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Pet Mode: always-on-top mini-window with 1 animated agent
- Token Health Bar above each character (with hysteresis bubbles)
- Custom Sprite Packs: drop a folder in `~/.pixel-agents/sprites/`
- Native OS notifications when an agent needs attention
- System tray with live agent count + Pet Mode toggle
- Global hotkey `Ctrl+Shift+P` to show/hide window
- JSONL replay at startup (30s lookback)
- Settings UI for context window size
- WebView2 bootstrapper bundled in the Windows installer
- Hooks server port fallback: tries `17317..=17326` and writes the chosen port to `~/.pixel-agents/hook-port`

### Fixed
- Sub-agent ID collision causing "Idle" label stuck forever
- Race TOCTOU on offsets producing duplicated events
- `session-map.json` corruption on concurrent writes
- `agentClosed` spurious emit at startup
- Memory leaks in long-running watcher (maps not cleaned)
- Frontend perf: throttled rAF, cached rect via `ResizeObserver`

### Security
- Constant-time comparison on hook token
- Host header validation against DNS rebinding (per bound port)
- Path canonicalization on sprite pack assets
- Hook token moved from global env var to `~/.pixel-agents/hook-token` (0600 on Unix)
