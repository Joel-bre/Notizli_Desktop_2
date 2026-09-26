# Notizli Desktop 2

The Notizli desktop recorder, rebuilt as a Tauri app with a Rust recording
engine, for Windows 10/11 and macOS 14.4+. It records a meeting on the user's
own computer (never as a bot): the room microphone on channel 0 and the call's
sound on channel 1. It saves the recording to disk as it goes, then uploads it
to https://notizli.ch, which transcribes it into minutes and action items.

It replaces the Electron recorder (`Joel-bre/notizli-desktop-selfhost`), whose
Windows capture recorded digital silence for Teams on laptop speakers. For the
investigation behind the design, see `native/win-audio-helper/FINDINGS.md` in
that repository.

To try it: [TESTING.md](TESTING.md).

## Layout

| Path | What |
|---|---|
| `crates/engine` | Recording engine: capture per platform, mixer, Opus encoder, crash-safe WebM writer, health report. |
| `crates/core` | notizli.ch API (pairing, upload), pairing links, device token in the OS credential store, the queue of recordings waiting for upload. |
| `crates/rec-cli` | `notizli-rec`: records to a file and prints a plain result. A hardware check without the app; `record-test.bat` runs it by double-click on Windows. |
| `app/src-tauri` | The app's Rust side: commands for the window, uploader, deep links, keep-awake, quit guard. |
| `app/ui` | The window: plain HTML/CSS/JS in the Notizli brand (no framework, no build step). |

## How recording works

- **Two sources, two clocks.** The microphone and the call are captured on their
  own threads, each pushing mono audio into a buffer. A mixer driven by the
  wall clock takes 10 ms from each every 10 ms (silence where a source has
  nothing), so a device disappearing never stalls the other channel. Buffers
  keep ~60 ms in reserve, trim 10 ms when a device runs fast, and re-prime when
  it runs dry. The channels never drift apart.
- **Windows.** Microphone: WASAPI shared capture of the chosen device, or of the
  default, reopened when the default changes or the device fails. The call:
  WASAPI *endpoint loopback of the speaker the meeting app is playing on*,
  chosen from the active audio sessions on every output device (call apps such
  as Teams/Zoom/Webex first, then browsers, then anything else, else the default
  speaker). It is re-checked every second and switched once chosen twice in a
  row. Per-app capture (process loopback) is not used: on the affected laptop it
  returned only digital silence, for every app.
- **macOS 14.4+.** Microphone: a Core Audio IOProc on the chosen or default
  input. The call: a private Core Audio process tap on everything the Mac plays
  (the "System Audio Recording" permission, not Screen Recording), read through
  an aggregate device that is rebuilt when the output device changes.
- **File.** 48 kHz Opus in WebM, 96 kbps stereo (48 kbps mono). Clusters of 2 s
  are written and synced as they complete, so a crash loses at most ~2 s; the
  app repairs interrupted files at the next start and queues them for upload.
  Everything is also kept in memory, so a failing disk doesn't lose the meeting.
- **Health report.** Every recording gets `<id>.health.json`: levels of both
  sides every 10 s, what was being recorded, device switches, gaps, and a plain
  verdict. It contains no audio and no words.

## Server contract (unchanged)

`POST /api/public/recorder/pair-preview`, `/pair` and `/upload` exactly as the
Electron recorder used them: `audio` (WebM), `title`, `started_at`,
`channel_layout` (`mic_remote` or `mono`), bearer device token. The base URL
can be changed at build time with `NOTIZLI_BASE_URL`.

## Security

- The device token lives in the Windows Credential Manager / macOS Keychain and
  is only used by the Rust side; the window never sees it.
- Pairing links are accepted only as `notizli-sh://pair?token=ccp_pair_…`, are
  previewed first ("Pair this recorder with …?", Cancel focused, names the
  current account when switching), and are refused during a recording.
- Strict CSP, no remote content, server text is always shown as text.

## Build

```
cargo test                      # engine + core + test tool (any OS)
cargo build -p notizli-rec      # the test tool
cd app && npm ci && npx tauri build   # the app (Windows or macOS)
```

On Linux the app crate needs the WebKitGTK development packages (see CI).
Recording itself only works on Windows and macOS.

CI (`.github/workflows/ci.yml`) runs the tests on all three systems and uploads
the Windows installer, the macOS app and the Windows test tool as artifacts on
every push. It never publishes a release.

## Releasing

Bump `version` in `Cargo.toml`, push, then run **Actions → Release (draft)**.
It builds the Windows installer and the macOS .dmg and attaches them to a
*draft* release; publish it by hand on GitHub when ready.

## Not done yet

- **Signing.** Windows builds are unsigned (SmartScreen warns). macOS builds are
  ad-hoc signed (Gatekeeper asks once, and Keychain may ask after each new
  build). Needs an Apple Developer ID (+ notarization) and ideally a Windows
  code-signing certificate, as secrets in this repository.
- **Automatic updates.** `tauri-plugin-updater` from GitHub Releases, once
  signing is set up (it needs its own update-signing key as a secret).
- **macOS universal build** (Intel Macs): currently Apple Silicon only.
- **Microphone clean-up.** No echo cancellation or noise suppression yet. The
  call's channel is the perfect echo reference for it later.
- **Separate-track transcription on the server** (room vs. online speakers).
- **On-device transcription** ("private mode"), for later.
