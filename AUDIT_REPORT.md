# Audit Report — pixel-agents-desktop

> Généré le 2026-05-15 · Comparaison HEAD (`679aa6f`) vs codebase complet

---

## Résumé exécutif

9 fichiers modifiés, 218 lignes ajoutées / 30 supprimées.  
**3 bugs critiques corrigés · 3 améliorations importantes appliquées · 4 items en backlog.**  
Tests : **23 passed, 0 failed.**

---

## Bugs critiques corrigés 🔴

### C1 — `panic!` dans `hooks_server.rs`
**Fichier :** `src-tauri/src/hooks_server.rs`

`stream.try_clone().unwrap_or_else(|_ | panic!(...))` pouvait tuer silencieusement le thread du serveur HTTP hooks (port 17317), coupant tous les events entrants de Claude Code sans aucun log.

**Fix :** `handle_connection` prend maintenant ownership du stream (plus de `&mut`). Le clone pour l'écriture est wrappé dans un `match` avec `warn!` + return propre. Un `BufWriter` assure le flush correct de la réponse.

---

### C2 — Commande `open_sessions_folder` non enregistrée
**Fichier :** `src-tauri/src/lib.rs`

`vscode-shim.ts` appelait `invoke("open_sessions_folder")` mais la commande n'était pas dans `tauri::generate_handler![]`. L'erreur était silencieusement avalée par `.catch(() => {})`.

**Fix :** Commande `open_sessions_folder` implémentée (cross-platform : `explorer` / `open` / `xdg-open`) et enregistrée dans le handler.

---

### C3 — `TokenUsage` écrasait `ToolUse` dans `jsonl_parser.rs`
**Fichier :** `src-tauri/src/jsonl_parser.rs`

Dans `extract_content_block`, un early return sur `TokenUsage` empêchait le parsing du `ToolUse` quand les deux étaient présents dans le même message assistant. L'agent restait visuellement idle alors qu'il utilisait un outil.

**Fix :** L'extraction du contenu actionnable (ToolUse / Text) est tentée en premier. `TokenUsage` n'est retourné que si aucun contenu actionnable n'est trouvé. 2 tests de non-régression ajoutés.

---

## Améliorations importantes appliquées 🟡

### I1 — Réponse HTTP JSON invalide dans `hooks_server.rs`
**Fichier :** `src-tauri/src/hooks_server.rs`

`Content-Type: application/json` mais body = `ok` (pas du JSON valide). Content-Length déclaré à 2 mais body = 4 bytes.

**Fix :** Body corrigé en `"ok"` (JSON string valide), Content-Length mis à 4, `BufWriter::flush()` ajouté.

---

### I2 — Accumulation infinie de `session-map.json`
**Fichiers :** `src-tauri/src/session_map.rs`, `src-tauri/src/file_watcher.rs`

Le fichier `~/.pixel-agents/session-map.json` n'était jamais nettoyé. Après des semaines d'usage, il contenait 1400+ entrées stales, causant un décalage des IDs (nouveau bug root cause : agents tous idle).

**Fix :** `session_map::remove_expired(ids)` appelé dans le thread expiry après chaque émission `agentClosed`.

---

### I6 — `always_on_top` persisté mais jamais appliqué
**Fichier :** `src-tauri/src/settings.rs`

La valeur était sauvegardée en store mais `win.set_always_on_top()` n'était jamais appelé. L'utilisateur devait redémarrer l'app pour voir l'effet.

**Fix :** `set_settings` applique immédiatement `always_on_top` sur la fenêtre principale après chaque changement.

---

## Fix session this session (non listé dans l'audit original) ✅

### S1 — Agents tous idle après accumulation de la session-map
**Fichiers :** `src-tauri/src/session_registry.rs`, `src-tauri/src/lib.rs`, `webview-ui/src/vscode-shim.ts`

Root cause : la `session-map.json` contenait des IDs allant jusqu'à 1407. `next_sequential = 1408` → nouveaux flows assignés IDs 1408-1410 côté Rust, mais frontend les assignait 1, 2, 3. Events émis sur des IDs inconnus du frontend.

**Fix :**
- `SessionMeta` expose maintenant `agent_id: Option<usize>`
- `list_sessions` accepte `SessionAgentMap` en state Tauri et peuple `agent_id` depuis la map persistée
- `session_agent_map` managé comme state avant `setup()` (élimine la race condition)
- `vscode-shim.ts` utilise `s.agent_id ?? (i + 1)` pour tous les agents (principaux + sous-agents)

---

## Backlog (non traité) 📋

| ID | Priorité | Description |
|----|----------|-------------|
| C4 | 🟡 | Mismatch d'index sub-agents si des sessions principales sont hors fenêtre `maxAgeHours: 2` |
| I3 | 🟢 | `bootstrapScheduled` non resetté entre appels refresh programmatiques |
| I7 | 🟡 | Port 17317 hardcodé sans fallback — hooks silencieusement désactivés si port occupé |
| I8 | 🟢 | Lock `session_to_agent` repris dans la boucle de lignes de `SubagentInit` (pattern fragile) |

---

## Fichiers modifiés

| Fichier | Changements |
|---------|-------------|
| `src-tauri/src/hooks_server.rs` | C1, I1 : panic → warn, owned stream, JSON valide, BufWriter |
| `src-tauri/src/lib.rs` | C2, S1 : open_sessions_folder, session_agent_map managé |
| `src-tauri/src/jsonl_parser.rs` | C3 : priorité ToolUse > TokenUsage + 2 tests |
| `src-tauri/src/session_map.rs` | I2 : `remove_expired()` ajouté |
| `src-tauri/src/file_watcher.rs` | I2 : appel `remove_expired` dans thread expiry |
| `src-tauri/src/settings.rs` | I6 : `always_on_top` appliqué en temps réel |
| `src-tauri/src/session_registry.rs` | S1 : `agent_id` dans SessionMeta, list_sessions enrichi |
| `webview-ui/src/vscode-shim.ts` | S1 : utilise `s.agent_id` pour tous les agents |

---

## Stats

```
Tests : 23 passed, 0 failed
cargo check : Finished (no errors, no warnings)
Lignes ajoutées : +218
Lignes supprimées : -30
```
