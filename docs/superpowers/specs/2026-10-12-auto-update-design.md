# Automatic updates — design

Agreed with Joel on 2026-10-12 (option A: quietly in the background, installed
at a calm moment).

## What the user gets

- Notizli checks for a new version when it starts and every 6 hours, and
  downloads it in the background.
- While not recording, the window shows a line at the top: **"Notizli 0.2.0 is
  ready."** → **Restart to update**; the menu bar / notification area menu
  shows **"Restart to update to 0.2.0"**. Hidden while recording.
- Started at login (`--hidden`) with an update available: it installs right
  away and restarts, before anything else. Started by hand: never restarts on
  its own.
- macOS: permissions survive updates (same Developer ID / Team ID).
- Only **published** GitHub releases reach users; drafts never do.

## How

- `tauri-plugin-updater` (v2), endpoint
  `https://github.com/Joel-bre/Notizli_Desktop_2/releases/latest/download/latest.json`,
  public key in `tauri.conf.json` (`plugins.updater.pubkey`). Windows installs
  with `installMode: "passive"` (progress bar, no questions).
- **Update key**: created once by Joel (`tauri signer generate`), private key
  and password as the secrets `TAURI_SIGNING_PRIVATE_KEY` and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, backed up in his password manager.
  Losing it means installed copies can never update again.
- **Release workflow**: builds with `createUpdaterArtifacts`, which signs the
  update files: macOS `Notizli.app.tar.gz` (+ `.sig`), Windows the NSIS
  installer (+ `.sig`). The release job attaches them and writes
  `latest.json` (version, date, per-platform URL and signature:
  `darwin-aarch64` → `Notizli.app.tar.gz`, `windows-x86_64` →
  `Notizli-Setup.exe`). CI test builds turn update files off (no key there).
- **App** (`src-tauri/src/updates.rs`): a thread checks at start and every 6 h
  (only while not recording), downloads, and keeps the downloaded update in
  memory. State `Ready { version }` is shown in the window (`get_state`) and
  the tray. `install_update` command (refused while recording) installs and
  restarts. At a `--hidden` start an available update is installed at once.
  Failures are logged and retried at the next check; never shown as errors.
- Version compare is the plugin's (semver from `Cargo.toml`'s workspace
  version), so each release needs a version bump.

## Testing

- Unit: none worth faking (the plugin does the work); the "not while
  recording" rule is a one-line guard.
- Manual (TESTING.md): publish 0.2.0, install it, publish 0.2.1 (draft →
  published), check the line appears within a start, click it, check the
  version afterwards; check a login start installs by itself.

## Out of scope

Release notes in the app, update channels (beta), Intel Macs.
