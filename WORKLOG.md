# The Slums — Worklog

**Date:** 2026-07-12
**Goal:** Lightweight launcher for The Slums WoW private server — pre-configured, zero user setup, with addon browser.

---

## 17:00 — Initial analysis of Relictum Launcher
- Explored existing Electron-based launcher at `/root/relictum-launcher`
- Electron 28 + React + Vite — ~600MB bundled, extension engine, integrity checks
- Decided it was overkill. User needed: install game files, manage addons, launch game. Nothing else.

## 17:30 — Built Tauri v2 replacement
- Created `/root/slums-launcher/` from scratch
- Rust backend (Tauri v2) + plain HTML/CSS/JS frontend — ~102MB AppImage
- Commands: `launch_game`, `get_realmlist`, `set_realmlist`, `clear_cache`, `list_installed_addons`, `fetch_addon_list`, `install_addon`, `delete_addon`, `select_game_path`
- Game launch auto-uses Wine on Linux, direct on Windows

## 18:00 — Client download via manifest
- Added `download_client` command — fetches manifest JSON, scans local files with SHA-256, downloads only changed/missing files
- Manifest format: `{ "version": "1.0", "files": [{ "path": "...", "sha256": "...", "size": N }] }`
- Added `get_client_status` — returns `not_installed` / `needs_update` / `ready`

## 18:30 — Server-side work
- Moved 27GB WoW client into `/root/acore-web/static/downloads/client/`
- Updated `CLIENT_PATH` in `app.py` to relative path
- Added public `/api/client/<path>` route (no auth required, for launcher)
- Added `/root/acore-web/scripts/generate-manifest.py` — regenerates manifest from client dir
- Updated `templates/downloads.html` for new launcher

## 19:00 — Pre-configured launcher
- Created `src-tauri/config.json` — embedded in binary via `include_str!`
- Config: manifest URL, addons URL, server address (wow.asslorde.com), account register URL
- Added `get_config` command
- Rewrote frontend: three-state UI — install / downloading / ready
- Added account register button in header → opens browser

## 19:30 — Rich addon browser
- Added installed/browse sub-tabs
- Search, sort (popular/newest/A-Z/Z-A), pagination (10 per page), detail modal
- Handles both Relictum-format addons.json and simple file-list format from webapp API
- Install downloads ZIP, extracts to Interface/AddOns
- TOC parsing for installed addon metadata (title, author, version)

## 20:00 — GitHub repos
- Created `joshhmann/the-slums-launcher` (Tauri app, 20 files)
- Created `joshhmann/the-slums-webapp` (Flask app, 20 files)
- Proper `.gitignore` — excludes node_modules, target/, gen/, client files, launcher binaries
- Both pushed as private repos

## 20:15 — White screen debugging
- Frontend files not embedding correctly — `frontendDist` was `"../src"`
- Fixed: changed to `"../dist"`, added `build:frontend` script to copy src→dist
- Cargo caches build script — must `touch build.rs` or `cargo clean` after frontend changes
- Verified with `tauri-codegen-assets` directory — files embedded as hashed/compressed assets

## 20:30 — Linux EGL crash
- User reported: "Could not create default EGL display: EGL_BAD_PARAMETER"
- `set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1")` in main.rs too late — EGL inits before Rust
- Fix: post-build script patches AppRun in the AppImage to export env vars before binary launch
- Script: `scripts/patch-appimage.sh` — extracts AppImage, injects exports, repacks
- Also set `GSK_RENDERER=cairo` as fallback

## 20:45 — Windows build
- `mkdir -p` and `cp -r` don't work on Windows — rewrote `build:frontend` as Node.js script
- `icon.ico` was a renamed PNG — not valid ICO format → RC.EXE compilation failed
- Fixed: used ImageMagick `convert` to generate proper multi-res ICO

## 21:00 — Tauri IPC not working on Windows
- `window.__TAURI__` global not available → all `invoke()` calls silently returned
- Fix: installed `@tauri-apps/api` npm package, converted to ES module imports
- Added import map in index.html: `"@tauri-apps/api/"` → `"./api/"`
- Build script copies API files from node_modules into `dist/api/` (Tauri blocks `dist/node_modules/`)

## 21:15 — Cross-platform build pipeline

```
npm run build
  ├─ build:frontend (node scripts/build-frontend.js)
  │    ├─ Copies src/ → dist/
  │    └─ Copies @tauri-apps/api → dist/api/
  └─ tauri build
       ├─ Runs beforeBuildCommand (npm run build:frontend)
       ├─ Compiles Rust (embeds frontend + config.json)
       └─ Creates bundles (AppImage, deb, rpm, .msi on Windows)
            └─ postbuild:appimage (Linux only) patches AppRun
```

## 21:30 — Final state

| Layer | What | Where |
|---|---|---|
| Launcher | Tauri v2, v1.0.1 | `/root/slums-launcher/` → `joshhmann/the-slums-launcher` |
| Webapp | Flask, serves client files + API | `/root/acore-web/` → `joshhmann/the-slums-webapp` |
| Client | 24.5 GB WoW 3.3.5a, 560 files | `/root/acore-web/static/downloads/client/` |
| Manifest | SHA-256 per-file, auto-generated | `/root/acore-web/static/downloads/manifest.json` |
| Server | AzerothCore WotLK | `/root/azerothcore-wotlk/` (not in repo) |

**User flow:** Download launcher → Click Install → Wait → Click Play → Game launches with realmlist pre-set.

## Known issues

1. **Cargo caches build script output** — after changing HTML/CSS/JS, must `cargo clean` or touch `build.rs` to force re-embed
2. **EGL on headless/no-GPU Linux** — mitigated by AppRun env vars, may still need `libegl1-mesa`
3. **Windows build not yet tested fully** — compiles, IPC fix in place, needs end-to-end test
4. **Addon API simple format** — `/api/addons` returns file list only; for rich metadata (images, descriptions), host a Relictum-format `addons.json`

---

## 21:45 — Independently hosted addon catalog

- Goal: own addon mirror, not dependent on Relictum
- Analyzed Relictum's pipeline: Warperia scrape → `_backup_addons/` → GitHub mirror → `build-addons.js` → `addons.json`
- GitHub mirror repo (`Litas-dev/Azeroth-Legacy-Addons-Mirror`) hosts 1.6GB of ZIPs, logos, screenshots — 698 addons
- Couldn't fork via API (403), cloned it directly instead
- Created `joshhmann/the-slums-addons` — empty public repo
- Cloned Relictum mirror (1.6GB, 3008 files), pushed to our repo as initial commit
- Wrote `scripts/build-addons.js` — reads per-addon `addon.json` metadata, finds ZIPs/logos, generates `addons.json` with our URLs
- Built `addons.json` — 698 addons, all pointing to `raw.githubusercontent.com/joshhmann/the-slums-addons/main/<addon>/<version>.zip`
- Custom "The Slums" addons (PlayerBots, etc.) still served from `wowslums.asslorde.com/downloads/addons/`
- Copied final `addons.json` to webapp's `static/downloads/`, committed all three repos

## 22:00 — Final architecture

| Repo | Purpose | Visibility |
|---|---|---|
| `joshhmann/the-slums-launcher` | Tauri v2 launcher | private |
| `joshhmann/the-slums-webapp` | Flask API + downloads | private |
| `joshhmann/the-slums-addons` | Addon ZIP mirror (698 addons, 1.6GB) | **public** |

**Addon flow:** Launcher fetches addons.json from webapp → webapp serves from `static/downloads/` → addon ZIPs download from GitHub raw CDN on our mirror.

**To update addons:** Drop new ZIP into `the-slums-addons/<folder>/`, add metadata `addon.json`, run `node scripts/build-addons.js`, push, copy `addons.json` to webapp. Can be automated with a GitHub Action.

## Known issues (updated)

1. **Cargo caches build script output** — after changing HTML/CSS/JS, must `cargo clean` or touch `build.rs` to force re-embed
2. **EGL on headless/no-GPU Linux** — mitigated by AppRun env vars, may still need `libegl1-mesa`
3. **Windows build not yet tested end-to-end** — compiles, IPC modules in place, needs full flow test with webapp running
4. **custom "The Slums" addons** — first 5 entries in addons.json still need their ZIPs hosted at `wowslums.asslorde.com/downloads/addons/`. The ones already there (PlayerBots.zip, etc.) work. Missing ones will fail to install silently.
5. **Webapp restart needed** — after copying new addons.json or manifest.json, the Flask app needs a restart
