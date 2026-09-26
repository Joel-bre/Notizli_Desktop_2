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
   `Notizli_0.1.0_x64-setup.exe` (again **More info → Run anyway**).
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
