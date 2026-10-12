# Automatic Updates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Notizli downloads new published versions in the background and installs them on "Restart to update" or at the next start at login, never while recording.

**Architecture:** `tauri-plugin-updater` 2.13 reads `latest.json` from the newest published GitHub release. `src-tauri/src/updates.rs` checks at start and every 6 h (5 min after a failure), keeps the downloaded `(Update, bytes)` in managed state, exposes `install_update`, and installs at once when started with `--hidden` (login) within 15 min of start. The window shows a banner, the tray an extra item. The release workflow signs update files with `TAURI_SIGNING_PRIVATE_KEY` and writes `latest.json`.

**Tech Stack:** Tauri 2.11, tauri-plugin-updater 2.13, GitHub Actions, jq.

Spec: `docs/superpowers/specs/2026-10-12-auto-update-design.md`.

---

### Task 1: Updater in the app
**Files:** create `app/src-tauri/src/updates.rs`; modify `app/src-tauri/Cargo.toml` (`tauri-plugin-updater = "2"`), `main.rs` (plugin, `mod updates`, manage `updates::Pending`, spawn, command), `commands.rs` (`update_ready` in `StateView`, `install_update`), `tray.rs` (an update item inserted/removed by `refresh`), `tauri.conf.json` (`plugins.updater` with endpoint, pubkey, `windows.installMode = "passive"`; `bundle.createUpdaterArtifacts = true`), `app/ui/index.html`, `app.js`.
- [ ] Write `updates.rs` (check → download → `Pending`; `install()` refuses while recording, installs, `app.restart()`).
- [ ] Wire plugin/state/thread/command; `StateView.update_ready: Option<String>`.
- [ ] Tray: `Restart to update to X` at the top while ready and not recording.
- [ ] Window: `#update-banner` ("Notizli X is ready." + "Restart to update"), hidden while recording.
- [ ] `cargo build -p notizli` clean.

### Task 2: Builds
**Files:** `.github/workflows/release.yml`, `.github/workflows/ci.yml`.
- [ ] Release build step gets `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)`; fails clearly if missing.
- [ ] Copy update files: macOS `Notizli.app.tar.gz` + `.sig`, Windows `Notizli-Setup.exe.sig`.
- [ ] Release job writes `latest.json` (`darwin-aarch64` → `Notizli.app.tar.gz`, `windows-x86_64` → `Notizli-Setup.exe`, URLs under `releases/download/v<version>/`) and uploads dmg, exe, tar.gz, latest.json.
- [ ] CI builds pass `--config '{"bundle":{"createUpdaterArtifacts":false}}'`.

### Task 3: Docs, verify, push
- [ ] README (Automatic updates, Releasing: the two new secrets and the key backup), TESTING.md (update test).
- [ ] `cargo test`, `cargo build -p notizli`, workflow YAML parses; commit; push.
- [ ] After Joel's key exists: put the public key in `tauri.conf.json`, run Release (draft), check `latest.json` and signatures in the draft.
