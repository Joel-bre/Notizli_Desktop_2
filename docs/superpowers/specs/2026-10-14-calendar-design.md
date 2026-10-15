# The recorder and the calendar — design

Agreed with Joel on 2026-10-14 (option 2, two releases). The server contract is
`docs/calendar-desktop-api.md` in `Joel-bre/callcap-claude`, copied in short
under "Server" below. The server ships first.

## Goal

Joel: *"I haven't seen much interaction between the desktop recorder and the
calendar."* Today the server links a desktop recording to a calendar meeting by
its start time, where nobody can see it. The desktop shows nothing. Two cases
go wrong: back-to-back meetings (nothing is linked, so the minutes get no
invitee names) and in-person meetings (no call app uses the microphone, so the
desktop never asks to record).

After this work:

- **Release 1 (0.3.0):** the "Record it?" box names the meeting. While
  recording, the window shows which meeting the recording is for, with "Not
  this meeting". The upload tells the server exactly which meeting it was, or
  none.
- **Release 2 (0.4.0):**
  - The window and the menu-bar menu show the meeting on now or the next one.
  - In-person and phone meetings get the same box at their start time.
  - A "Connect your calendar" line appears when the calendar is offered but not connected.

## Rules

- **Never invitee names.** The server sends titles and times only (decision C1).
- **Nothing starts a recording by itself** (Art. 179ter StGB). Every box still
  needs a click on Record.
- **Meetings live in memory only.** The exception is the recording's own
  meeting: its id and display line are saved in that recording's local `.json`
  until the upload is accepted, and deleted with it. Meeting titles never go to
  the log; log lines say "calendar: 3 meetings, one on now" at most.
- **The calendar never delays or blocks a recording.** Recording starts first.
  The meeting is attached when the server answers. If it doesn't answer within
  5 s, nothing is attached and the server falls back to its time match.
- **The upload title stays the desktop's default** ("Zoom call — 14 Oct 2026,
  15:03"). If the calendar title were sent as the title, the server would treat
  it as typed by the user and would never replace it with "Customer — Project —
  AI name".

## Decisions (defaults Claude chose; Joel can still change them)

| # | Question | Default |
|---|---|---|
| D1 | Meeting titles on the computer? | Yes, in memory only (see Rules). |
| D2 | When does the box ask for an in-person meeting? | At the start time, still shown up to 2 minutes later (a laptop waking up), once per meeting. It goes away after 60 s like the call box. |
| D3 | Two meetings fit (server `now` is null)? | The box shows today's text with no name. The server's time match then decides, which for two meetings means none. |
| D4 | Which switch controls the in-person box? | The existing one, renamed "Ask me to record when a call or meeting starts". |
| D5 | "Not this meeting" | One click, no undo. The line then reads "Not linked to a calendar meeting." |
| D6 | Picking a different meeting by hand | Not now (the web has no picker either). |

## Server (summary of the contract)

`GET /api/public/recorder/upcoming-meetings`, Bearer device token →

```json
{ "available": true, "connected": true, "providers": [...], "now": "g:abc" | null,
  "meetings": [{ "id": "g:abc", "provider": "google", "title": "Weekly sync",
                 "start": "…Z", "end": "…Z", "joinType": "teams" | "zoom" | "meet" | "webex" | "other" | null }] }
```

- `available`: the calendar is offered to this account.
- `now`: the meeting the server would link a recording started now to.
- `meetings`: meetings in progress or starting within the next 3 hours, at most 20.
- Any non-200 answer, including 404 from an older server, counts as "no calendar". The desktop shows no error.

`POST /upload` gains an optional field `calendar_event_id`: a meeting id, or
`none` for "Not this meeting". Leaving it out means the time match, as before.

## Release 1 (0.3.0)

**Box (call detection).**
- The box appears at once, exactly as today.
- In parallel, the app asks the server (reusing a list up to 2 minutes old).
- If `now` names a meeting and the box is still showing, the box changes to:
  - **Weekly sync · 14:00 · Teams**
  - You're in a call (Zoom). Record it?
- Record starts the recording for that meeting.
- If the server is slow or names nothing, the box stays as it is today.

**Start from the window or the menu bar.** Recording starts at once. The app
then asks the server for `now` and attaches that meeting if there is one.

**While recording**, under the title:
- *For: Weekly sync · 14:00 · Teams* — **Not this meeting**
- After a click: *Not linked to a calendar meeting.*
- With no meeting known: no line at all.

**Upload.** It sends `calendar_event_id` = the meeting id, `none`, or nothing.

**Display line.** Title · local time (HH:MM) · how to join. "How to join" is
Teams, Zoom, Google Meet, Webex, Online (another link), or "In person or by
phone" (no link). "(no title)" shows as "Untitled meeting".

## Release 2 (0.4.0)

**Refresh.** While paired, the list is fetched every 5 minutes. It is also
fetched when the window gets focus, unless the list is under 2 minutes old.

**Idle window**, above the title field. One of these shows, or nothing when
the calendar isn't offered or the server didn't answer:
- A meeting is on now: a tick-box **"This recording is for: Weekly sync ·
  14:00 · Teams"**, ticked. This is the same wording as the web recorder.
  Unticked, Start recording sends `none`.
- Otherwise, a meeting later within 3 hours: *Next meeting: Acme review ·
  16:00 · Zoom* (information only).
- Offered but not connected: *Connect your calendar so recordings are linked
  to their meeting.* **Connect…** opens notizli.ch/settings in the browser.

**Menu bar.** A greyed-out first line, "Now: Weekly sync · 14:00 · Teams" or
"Next: Acme review · 16:00 · Zoom". It is hidden when there is neither.
"Start recording" from the menu uses `now`, as in release 1.

**In-person box.** A meeting with no video link (`joinType` null) gets a box
when all of these hold:
- the meeting started less than 2 minutes ago;
- the app is paired and the switch is on;
- nothing is being recorded and no box is showing;
- this meeting hasn't been asked about since the app started.

The box:
- **Weekly sync · 14:00 · In person or by phone**
- Starting now. Record it?
- Buttons: Record / Not now. It goes away after 60 s.

Record starts a "Desktop recording — <date>" recording for that meeting. A
call detected meanwhile replaces the box with the call box, as today.

## Testing

- **Unit (`crates/core`):**
  - parsing the server answer, including `{}` and unknown fields;
  - the display line (time zone offset, every `joinType`, no title);
  - `now_meeting`, `next_meeting`, `in_person_due` (window edges, asked once);
  - the upload sends `calendar_event_id` only when set;
  - an old recording `.json` without the new fields still loads.
- **Manual, debug builds:**
  - `NOTIZLI_FAKE_UPCOMING=<file.json>` makes the app read that file instead of asking the server.
  - Combined with `NOTIZLI_FAKE_CALL`, it shows every box without a real call or calendar.
- **Live (Joel):**
  - one Teams call from a calendar invite (the box names it, the minutes name the invitees);
  - one back-to-back pair;
  - one in-person meeting (release 2).
