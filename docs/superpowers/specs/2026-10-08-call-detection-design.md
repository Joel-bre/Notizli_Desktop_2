# Call detection: "Record this call?" — design

Agreed with Joel on 2026-10-08. Calendar reminders are a separate piece of work
(notizli.ch first, then combined with this); not part of this design.

## What the user gets

- Notizli starts at login and lives in the menu bar (macOS) / notification
  area (Windows). Closing the window hides it; recording continues.
- When a call app or a browser has been using the microphone for 10 s and
  Notizli is not recording, a small box appears at the top right of the screen:
  **"You're in a call (Microsoft Teams). Record it?"** → **Record** / **Not now**.
  It closes by itself after 60 s. After *Not now* (or the timeout) it does not
  ask again until that app has released the microphone.
- While recording, when every call app or browser has released the microphone
  for 5 s, the box shows **"The call ended. Finishing in 60 seconds"** →
  **Finish now** / **Keep recording**. Without an answer the recording is
  finished and uploaded exactly as if *Finish and transcribe* had been clicked.
  This only happens if a call app or browser held the microphone at some point
  during the recording; a room-only recording is never ended by it.
- Never records without a click (Swiss law: recording a conversation without
  consent is an offence, Art. 179ter StGB). There is no "always record" option.

Decisions (Joel): background mode on by default (A), ask every time (A),
countdown at the end (B), call apps and browsers trigger it (A), the prompt is
a small Notizli box rather than a system notification (1).

## Which apps count

Detection looks at **which processes are using the microphone right now**,
never at window titles, URLs or audio content.

- **Call apps**: Teams, Zoom, Webex, Slack, WhatsApp, Discord, Skype, FaceTime.
- **Browsers**: Chrome, Edge, Firefox, Safari, Brave, Opera, Vivaldi, Arc,
  Comet. The box names the browser ("Your browser (Google Chrome) is using the
  microphone. Record it?"), since the site is unknown.
- Everything else is ignored, and so is Notizli itself.

On Windows a process counts if it or one of its ancestors matches (Teams and
browsers capture from child processes), as the speaker chooser already does.
On macOS the match is on the bundle id, with helper ids mapped to their app
(`com.google.Chrome.helper` → Google Chrome; `com.apple.WebKit.GPU` → Safari).

The app lists for Windows (`MEETING_APPS`, `BROWSERS` in
`capture/windows.rs`) move to one shared module used by both the speaker
chooser and detection, with the macOS bundle ids next to them.

## Components

1. **`notizli_engine::mic_users()`** (new, `crates/engine/src/detect/`):
   returns the apps using a microphone now: `Vec<MicUser { app: String, kind:
   Call | Browser }>`, own process excluded.
   - macOS 14.4+: Core Audio process objects
     (`kAudioHardwarePropertyProcessObjectList`, then per process
     `kAudioProcessPropertyIsRunningInput`, `…BundleID`, `…PID`). No permission
     needed.
   - Windows: audio sessions on every capture endpoint (`IAudioSessionManager2`,
     the same calls already used on speakers), state *Active*, process id →
     executable and ancestors via `sysinfo`.
   - Other OS: always empty.
2. **`CallWatcher`** (engine, pure logic, no OS calls): fed a snapshot of
   `mic_users()`, the current time and whether Notizli is recording; returns
   at most one event: `AskToRecord { app, kind }`, `CallEnded`, `HidePrompt`.
   Holds the timings (10 s to ask, 60 s prompt lifetime, 5 s grace at the end)
   and the "already asked for this app" memory. Unit-tested with a fake clock
   on every OS.
3. **Detector thread** (app, `src-tauri/src/detect.rs`): every 2 s calls
   `mic_users()`, feeds `CallWatcher`, and acts on events: shows/hides the
   prompt window, starts a recording (title `"<App> call — <date, time>"`), or
   starts the end countdown. Runs only while paired and the setting is on.
   Logs changes in the set of detected apps (app names only).
4. **Prompt window** (`app/ui/prompt.html`, label `prompt`): 360×120,
   no frame, always on top, not focused when shown, not in the taskbar,
   positioned at the top right of the main screen's work area, and
   content-protected so it is hidden from screen sharing where the OS allows
   it. Two modes: *ask* and *countdown*. Buttons call commands
   (`prompt_record`, `prompt_dismiss`, `prompt_finish_now`, `prompt_keep`).
   Its own capability file allows only those.
5. **Tray / menu bar** (Tauri `tray-icon` feature): Start recording (or
   Finish recording while recording), Open Notizli, Quit. Quit while recording
   asks first, as today.
6. **Background mode**: `tauri-plugin-autostart`, enabled on first launch;
   the window's close button hides the window instead of quitting (the current
   "ask before closing while recording" becomes unnecessary for close, kept for
   Quit). On macOS the Dock icon is shown only while the window is open.
7. **Settings** (in the main window): "Ask me to record when a call starts"
   (default on) and "Open Notizli when I log in" (default on).

## Behaviour details

- *Record* in the box: starts recording, hides the box, opens the main window
  without stealing focus from the call where possible (it shows the timer).
- If the app releases the microphone while the *ask* box is showing, the box
  closes (`HidePrompt`).
- The end countdown is shown in the box; *Keep recording* lets the recording
  run on and the countdown appears again only after a call app or browser
  takes the microphone again and then releases it. Finishing from the countdown uses
  the same code path as the *Finish and transcribe* button (upload, unsent
  queue when offline).
- If the user finishes or discards manually, any open box closes.
- Not paired: no detection (nowhere to upload).
- Detection never touches audio and never starts the engine's capture by
  itself.

## Testing

- Unit tests (any OS): `CallWatcher` timelines (ask after 10 s, not before;
  one ask per call; timeout counts as Not now; brief release < 5 s does not
  end the call; countdown only when a call app was seen during the recording;
  Keep recording re-arms), app classification for Windows exe chains and
  macOS bundle ids.
- Manual, added to TESTING.md: Teams app, Zoom, Google Meet in Chrome and in
  Safari; Not now; let the countdown run out; Keep recording; share the screen
  while the box shows; restart the computer and check Notizli is in the menu
  bar / notification area.

## Out of scope

Calendar reminders (separate notizli.ch work, combined later), knowing which
website a browser call is on, "always record" or "never ask for this app".
