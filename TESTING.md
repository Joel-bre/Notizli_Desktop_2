# Testing the new recorder

Three tests, in this order. Each one only needs a normal call. Send the results
(or what went wrong) back to Claude.

All downloads come from GitHub → **Actions** → the latest green **CI** run →
**Artifacts** at the bottom of the page:
https://github.com/Joel-bre/Notizli_Desktop_2/actions
(you must be logged in to GitHub). They are test builds, not releases: nobody
gets updated automatically.

Keep the old "Notizli Self-Hosted" app installed for now.

---

## A. Windows: the recording test (5 minutes)

This checks that the new engine hears both sides on your laptop, before the app
is involved.

1. Download **notizli-record-test-windows-…** and unzip it (right-click → *Extract All*).
2. Join a Teams call and let the call play on the **laptop speakers**, the way
   it used to fail. Ask the other person to talk (or play a video in the call).
3. Double-click **record-test.bat**. If Windows says "Windows protected your PC",
   click **More info → Run anyway** (the test file isn't signed).
4. The black window shows live levels for **you** and **the call**, and what it
   is recording (e.g. "Microsoft Teams — Speakers (Realtek(R) Audio)").
5. Optional but useful: after about one minute, connect your AirPods, and after
   another minute disconnect them. The recorder should follow.
6. After 3 minutes (or press Enter) Notepad opens with the result.
   **Paste the whole text to Claude.** The recording (`notizli-test-….webm`)
   is next to it if you want to listen.

What a good result looks like: `RESULT: Both sides recorded.` and a "call"
column with numbers around -20 to -50 while the other side talked
(-120 means silence).

---

## B. Windows: the app

1. Download **notizli-windows-installer-…**, unzip, run
   `Notizli_<version>_x64-setup.exe` (again **More info → Run anyway**).
2. The app opens on "Connect this app to your account". Click
   **Pair with my account**: notizli.ch/pair opens in the browser. Click
   **Pair this device** there; the browser asks to open Notizli → allow.
   - The app asks **"Pair this recorder with <your email>?"**. Check the email,
     click **Pair**.
   - If the old app opens instead, copy the token from the web page and paste it
     into the new app's "paste a pairing token" field.
3. Enter a title, click **Start recording**, talk for a minute during a call.
   Check: the timer runs, both meters move, and it says what it hears.
4. Click **Finish and transcribe**. It shows the upload progress, then
   **Uploaded — transcription started** → **Open meeting**. Under the title it
   says whether both sides were heard ("Check: Both sides recorded.") or, in a
   red box, what was missing.
5. Also worth a try:
   - Close the window while recording: it must ask first.
   - Turn Wi-Fi off, record 30 seconds, finish: the recording must stay in the
     "haven't uploaded yet" list. Turn Wi-Fi on and click **Upload now**.

---

## C. Mac (M4 MacBook Air)

1. Download **notizli-macos-…** and unzip. Open the `.dmg` and drag **Notizli**
   into Applications. (If the .dmg is missing, unpack `Notizli-macOS.tar.gz`
   instead.)
2. The first time, macOS blocks it because it isn't signed yet: open
   **System Settings → Privacy & Security**, scroll down, click **Open Anyway**.
3. Pair as on Windows.
4. Start a recording during a call. macOS asks to allow the **Microphone** and
   **System Audio Recording**: allow both. If you clicked "Don't allow" by
   mistake: System Settings → Privacy & Security → Microphone / Screen & System
   Audio Recording → turn Notizli on.
5. Finish, check the meeting on notizli.ch. Try once with AirPods too.

---

## D. Call detection (Mac and Windows)

Notizli now starts with the computer and has an icon in the menu bar (Mac) or
next to the clock (Windows). Closing its window only hides it.

1. Join a Teams or Zoom call (the app, not the browser). After about 10
   seconds a small box appears at the top right: **"You're in a call (…).
   Record it?"**. Click **Record**.
2. Talk for a minute, then leave the call. About 5 seconds later the box says
   **"The call ended. Finishing in 60 seconds"**. Let it run out: the
   recording finishes and uploads by itself.
3. Join another call and click **Not now**: it must not ask again during that
   call.
4. Try a call in the browser (Google Meet in Chrome, or Teams in Safari): the
   box says "Your browser (…) is using the microphone".
5. Start a recording during a call, leave the call, and click **Keep
   recording** in the countdown: the recording continues.
6. Share your screen in a call while the box is showing, and ask the other
   person whether they can see the box (they shouldn't).
7. Restart the computer: the Notizli icon must be there without opening it.

Windows: `notizli-rec calls` (in the test tool's folder, from a command
prompt) prints which call apps it sees using the microphone.

## E. Automatic updates (once two versions are published)

1. Install the published version from the download link.
2. Publish the next version on GitHub (Releases → the draft → Publish).
3. Quit Notizli and open it again (or wait up to 6 hours). Within a minute the
   window says **"Notizli X is ready."** → click **Restart to update**.
   Notizli restarts; the version at the bottom left is the new one.
4. Mac: start a recording afterwards. macOS must **not** ask for the
   microphone or system audio again.
5. Start a recording while an update is ready: the line disappears and comes
   back after finishing.

## If something goes wrong

Click **Diagnostics** (bottom left, next to the version). It opens a folder
with `notizli.log` (what the app did) and the health report of each recording.
Send those files to Claude.

## Where the app keeps things

| | Windows | Mac |
|---|---|---|
| Recordings not yet uploaded | `%APPDATA%\ch.notizli.recorder\unsent` | `~/Library/Application Support/ch.notizli.recorder/unsent` |
| Health report of each recording | `...\ch.notizli.recorder\logs` | `.../ch.notizli.recorder/logs` |

The health report (`*.health.json`) says, every 10 seconds, how loud each side
was and what was being recorded. It contains no audio and no words. If a
recording sounds wrong, send it to Claude.

---

## Calendar (0.3.0)

Needs a calendar connected on notizli.ch (Settings → Calendar) and a meeting
with someone else invited, with a Teams/Zoom/Meet link.

1. Join the meeting's call within 10 minutes of its start. The box says
   "<meeting> · <time> · Teams" and "You're in a call (Teams). Record it?".
2. Record, talk for a minute, finish. On notizli.ch the meeting page says
   "From your calendar: <meeting>", and the minutes use the invitees' names.
3. Two meetings back to back (the first still running when the second
   starts): join the second. The box names the second one.
4. Start a recording from the window during a meeting and click
   "Not this meeting". On notizli.ch the recording is not linked.

### Calendar, debug build (developer checks, not run before 0.3.0)

Nobody has watched these screens yet: when Claude built 0.3.0, the installed
Notizli was running (same identifier, data folder and keychain token) and a
recording would have needed a microphone permission prompt. Quit the
installed Notizli first. Write `/tmp/upcoming.json`, with `start` a few
minutes before now (UTC) and `end` 30 minutes after:

```json
{ "available": true, "connected": true, "providers": [], "now": "g:demo",
  "meetings": [{ "id": "g:demo", "provider": "google", "title": "Weekly sync",
                 "start": "2026-10-14T12:00:00Z", "end": "2026-10-14T12:30:00Z", "joinType": "teams" }] }
```

Then `cd app && NOTIZLI_FAKE_CALL=120 NOTIZLI_FAKE_UPCOMING=/tmp/upcoming.json npx tauri dev`:

1. After ~10 s the box says "You're in a call (Zoom)." and almost at once
   changes to "Weekly sync · HH:MM · Teams" / "In a call (Zoom). Record it?".
2. Record → the window shows "For: Weekly sync · HH:MM · Teams — Not this meeting".
   While it records, `unsent/<id>.json` holds `calendar_ref` and `calendar_line`.
3. Not this meeting → "Not linked to a calendar meeting." Discard.
4. `"now": null` → the box keeps "You're in a call (Zoom)." / "Record it?" and
   the recording shows no meeting line.
5. A 60-character title → the box shows one line ending in "…"; the buttons
   stay visible.
6. `notizli.log` never contains "Weekly sync".

## Calendar (0.4.0)

1. With a meeting in the next 3 hours: the window says "Next meeting: …" and
   the menu bar's first line "Next: …". During it: "This recording is for: …"
   (ticked) and "Now: …".
2. A meeting with no video link (an office visit) with someone invited: at its
   start time the box asks "Starting now. Record it?". Record, finish; on
   notizli.ch the recording is linked to that meeting.
3. On an account without a calendar connected: the window suggests connecting.

### Calendar 0.4.0, debug build (developer checks, not run before 0.4.0)

Not watched yet either: the installed Notizli was running again when Claude
built 0.4.0. Quit it first. Use the `/tmp/upcoming.json` above. The app reads
the file again about every 15 s. While unpaired, add `NOTIZLI_FAKE_CALL=0`:
call detection is otherwise off and hides every box at once.

`cd app && NOTIZLI_FAKE_CALL=0 NOTIZLI_FAKE_UPCOMING=/tmp/upcoming.json npx tauri dev`:

1. `"now": "g:demo"` → within 15 s the window shows the ticked "This recording
   is for: Weekly sync · …", and the menu bar's first line is "Now: Weekly sync · …".
2. Untick and record → "Not linked to a calendar meeting." Discard.
3. `"now": null`, `start` 1 hour ahead → "Next meeting: Weekly sync · …" and
   "Next: …" in the menu.
4. `"connected": false, "meetings": []` → the connect line; **Connect…** opens
   notizli.ch/settings.
5. `"available": false` → nothing calendar-related.
6. `"now": null`, `"joinType": null`, `start` 1 minute ahead → at the start
   time the box says "Weekly sync · HH:MM · In person or by phone" /
   "Starting now. Record it?"; left alone, it goes after 60 s and doesn't come
   back. With a new id and start, **Record** → "Desktop recording — …" with
   "For: Weekly sync · … · In person or by phone". Switch off → no box.

---

## Echo cancellation (0.5.0)

On the laptop speakers, no headphones:

1. Record a call (or play a YouTube video of someone talking) and stay silent
   for 30 s, then talk alone for 20 s, then talk over it for 30 s.
2. On notizli.ch the minutes should give the other side's words to the other
   side, not to you, and your sentences to you.
3. The health report (Diagnostics) says `"echo": {"on": true, "removed_db": …}`.
4. Untick "Remove the call's sound from my microphone" and repeat: the report
   says `"on": false` and the microphone hears the call again.
5. With headphones nothing should change: your voice sounds as before.

Without the app: `notizli-rec record` prints "Echo cancellation: on, the
microphone was N dB quieter while the call played"; `--no-echo-cancel` turns it off.
