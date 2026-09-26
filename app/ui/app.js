// Notizli desktop recorder — the window. All work happens in the Rust side;
// this file shows state and forwards button presses. Server-provided text
// (emails, titles, errors) is always set with textContent, never as HTML.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const screens = ["s-unpaired", "s-idle", "s-starting", "s-recording", "s-uploading", "s-done", "s-error"];

let S = null; // last state from Rust
// What the window is doing beyond the Rust state: null (follow state),
// "starting", "recordUnpaired", {uploading: id}, {done: view}, {error: view}.
let mode = null;
let timerBase = { ms: 0, at: 0 };
let micsLoaded = false;

// ---- helpers -----------------------------------------------------------------

function show(id) {
  for (const s of screens) $(s).hidden = s !== id;
}

function fmtClock(ms) {
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${String(m).padStart(2, "0")}:${sec}`;
}

function fmtSize(bytes) {
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1e3))} KB`;
}

function fmtDate(iso) {
  const d = new Date(iso);
  if (isNaN(d)) return "";
  return d.toLocaleString(undefined, { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
}

function errText(e) {
  return typeof e === "string" ? e : (e && e.message) || "Something went wrong.";
}

let toastTimer = null;
function toast(text) {
  const t = $("toast");
  t.textContent = text;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), 6000);
}

// A confirmation dialog. Cancel has the focus, so Enter never confirms by accident.
function confirmDialog({ title, text, note, ok = "OK", cancel = "Cancel" }) {
  return new Promise((resolve) => {
    $("dialog-title").textContent = title;
    $("dialog-text").textContent = text || "";
    $("dialog-note").textContent = note || "";
    $("dialog-note").hidden = !note;
    $("dialog-ok").textContent = ok;
    $("dialog-cancel").textContent = cancel;
    $("dialog").hidden = false;
    const done = (v) => {
      $("dialog").hidden = true;
      $("dialog-ok").onclick = $("dialog-cancel").onclick = null;
      document.removeEventListener("keydown", onKey);
      resolve(v);
    };
    const onKey = (e) => {
      if (e.key === "Escape") done(false);
    };
    document.addEventListener("keydown", onKey);
    $("dialog-ok").onclick = () => done(true);
    $("dialog-cancel").onclick = () => done(false);
    $("dialog-cancel").focus();
  });
}

// ---- state ---------------------------------------------------------------------

async function refresh() {
  try {
    S = await invoke("get_state");
  } catch (e) {
    console.error(e);
    return;
  }
  render();
}

let refreshQueued = false;
function refreshSoon() {
  if (refreshQueued) return;
  refreshQueued = true;
  setTimeout(() => {
    refreshQueued = false;
    refresh();
  }, 150);
}

function render() {
  if (!S) return;
  $("version").textContent = `Notizli ${S.version}`;
  renderUnsent();

  if (S.recording) {
    mode = mode === "starting" ? null : mode;
    renderRecording();
    return show("s-recording");
  }
  if (mode === "starting") return show("s-starting");
  if (mode && mode.uploading) return renderUploading();
  if (mode && mode.done) return renderDone(mode.done);
  if (mode && mode.error) return renderError(mode.error);

  if (!S.paired && mode !== "recordUnpaired") {
    $("token-msg").hidden = true;
    return show("s-unpaired");
  }
  renderIdle();
  show("s-idle");
}

function renderUnsent() {
  const list = S.unsent || [];
  for (const box of document.querySelectorAll("[data-unsent]")) {
    box.hidden = list.length === 0;
    box.replaceChildren();
    if (!list.length) continue;
    const head = document.createElement("div");
    head.className = "unsent-head";
    const text = document.createElement("span");
    const uploading = S.upload ? " — uploading now" : "";
    text.textContent =
      (list.length === 1 ? "1 recording hasn't uploaded yet" : `${list.length} recordings haven't uploaded yet`) + uploading;
    const actions = document.createElement("span");
    actions.className = "unsent-actions";
    if (S.paired) actions.append(linkButton("Upload now", () => invoke("upload_now", { id: null }).then(() => toast("Uploading…"))));
    actions.append(linkButton("Show files", () => invoke("show_in_folder", { id: null }).catch((e) => toast(errText(e)))));
    head.append(text, actions);
    const ul = document.createElement("ul");
    for (const r of list) {
      const li = document.createElement("li");
      const t = document.createElement("div");
      t.className = "item-title";
      t.textContent = r.title;
      const meta = document.createElement("div");
      meta.className = "item-meta";
      const bits = [fmtDate(r.started_at), fmtClock(r.duration_ms), fmtSize(r.size)];
      if (r.recovered) bits.push("recovered after the app stopped");
      if (S.upload && S.upload.id === r.id && S.upload.total) bits.push(`uploading ${Math.round((100 * S.upload.sent) / S.upload.total)}%`);
      meta.textContent = bits.filter(Boolean).join(" · ");
      li.append(t, meta);
      if (r.status === "too_large") {
        const e = document.createElement("div");
        e.className = "item-error";
        e.textContent = "Too large for Notizli (200 MB max). Save a copy and keep it.";
        li.append(e);
      } else if (r.last_error) {
        const e = document.createElement("div");
        e.className = "item-error";
        e.textContent = r.last_error;
        li.append(e);
      }
      const act = document.createElement("div");
      act.className = "item-actions";
      if (S.paired && r.status === "ready") act.append(linkButton("Upload", () => invoke("upload_now", { id: r.id }).then(() => toast("Uploading…"))));
      act.append(linkButton("Save a copy…", () => saveCopy(r.id)));
      act.append(
        linkButton("Delete", async () => {
          const ok = await confirmDialog({
            title: "Delete this recording?",
            text: `"${r.title}" will be deleted from this computer. It hasn't been uploaded, so it will be gone for good.`,
            ok: "Delete",
          });
          if (ok) invoke("discard_unsent", { id: r.id }).then(refresh).catch((e) => toast(errText(e)));
        }),
      );
      li.append(act);
      ul.append(li);
    }
    box.append(head, ul);
  }
}

function linkButton(label, onClick) {
  const b = document.createElement("button");
  b.className = "link";
  b.textContent = label;
  b.onclick = onClick;
  return b;
}

function renderIdle() {
  $("paired-row").hidden = !S.paired;
  $("unpaired-banner").hidden = S.paired;
  $("unpair-btn").hidden = !S.paired;
  $("paired-label").textContent = S.email ? `Paired — ${S.email}` : `Paired — ${S.label || "this computer"}`;
  $("platform-hint").textContent =
    S.os === "macos"
      ? "The first time you record, macOS asks to allow the Microphone and System Audio Recording. Both are needed to record the two sides."
      : "Nothing to set up: Notizli records your microphone and the speaker your meeting app plays on.";
  if (!micsLoaded) loadMics();
}

async function loadMics() {
  micsLoaded = true;
  const sel = $("device");
  try {
    const mics = await invoke("list_mics");
    const current = S && S.mic_device ? S.mic_device : "";
    sel.replaceChildren();
    const def = document.createElement("option");
    def.value = "";
    const defMic = mics.find((m) => m.is_default);
    def.textContent = defMic ? `Default microphone (${defMic.name})` : "Default microphone";
    sel.append(def);
    for (const m of mics) {
      const o = document.createElement("option");
      o.value = m.id;
      o.textContent = m.name;
      sel.append(o);
    }
    sel.value = mics.some((m) => m.id === current) ? current : "";
  } catch (e) {
    console.warn("microphones", e);
  }
}

function renderRecording() {
  const r = S.recording;
  timerBase = { ms: r.elapsed_ms, at: performance.now() };
  $("rec-title").textContent = r.title;
  $("rec-label").textContent = r.layout === "mic_remote" ? "Recording — both sides" : "Recording — microphone only";
  $("mic-name").textContent = r.labels.microphone || "";
  $("other-name").textContent = r.layout === "mic_remote" ? (r.labels.hearing ? `Hearing: ${r.labels.hearing}` : "") : "Not recorded";
  $("silent-warning").hidden = !r.labels.warning;
}

function renderUploading() {
  const id = mode.uploading;
  const u = S.upload && S.upload.id === id ? S.upload : null;
  const title = (S.unsent.find((x) => x.id === id) || {}).title || "";
  $("upload-title").textContent = title;
  if (u && u.total) {
    const pct = Math.min(100, Math.round((100 * u.sent) / u.total));
    $("upload-label").textContent = `Uploading ${pct}%`;
    $("upload-fill").style.width = `${pct}%`;
    $("upload-detail").textContent = `${fmtSize(u.sent)} of ${fmtSize(u.total)}. The recording is safe on this computer.`;
  } else {
    $("upload-label").textContent = "Uploading…";
    $("upload-fill").style.width = "0%";
  }
  show("s-uploading");
}

function renderDone(v) {
  $("done-title").textContent = v.meeting_id ? "Uploaded — transcription started" : "Saved on this computer";
  $("done-text").textContent = v.meeting_id
    ? "Minutes and action items appear on the meeting page once the transcript is ready."
    : "Pair this recorder with your Notizli account and it uploads the recording right away.";
  $("open-meeting-btn").hidden = !v.meeting_id;
  $("done-pair-btn").hidden = !!v.meeting_id || S.paired;
  $("open-meeting-btn").onclick = () => invoke("open_meeting", { meetingId: v.meeting_id }).catch((e) => toast(errText(e)));
  show("s-done");
}

function renderError(v) {
  $("error-title").textContent = v.title || "That didn't work";
  $("error-msg").textContent = v.message || "";
  $("error-note").textContent = v.note || "";
  $("error-note").hidden = !v.note;
  $("error-retry-btn").hidden = !v.retryId;
  $("error-pair-btn").hidden = !v.pair;
  $("error-save-btn").hidden = !v.saveId;
  $("error-retry-btn").onclick = () => {
    mode = { uploading: v.retryId };
    invoke("upload_now", { id: v.retryId });
    render();
  };
  $("error-save-btn").onclick = () => saveCopy(v.saveId);
  show("s-error");
}

async function saveCopy(id) {
  try {
    const path = await invoke("save_copy", { id });
    if (path) toast(`Saved a copy: ${path}`);
  } catch (e) {
    toast(errText(e));
  }
}

// ---- meters and timer ------------------------------------------------------------

function level(peak) {
  if (!peak || peak <= 0) return 0;
  const db = 20 * Math.log10(peak);
  return Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
}

setInterval(() => {
  if (S && S.recording) $("timer").textContent = fmtClock(timerBase.ms + (performance.now() - timerBase.at));
}, 250);

// ---- pairing -------------------------------------------------------------------

async function askPair(req) {
  if (!req) return;
  const switching = req.current_email && req.current_email !== req.email;
  const ok = await confirmDialog({
    title: switching ? "Switch account?" : "Pair this recorder?",
    text: switching
      ? `This recorder is currently paired with ${req.current_email}. Switch to ${req.email}?`
      : `Pair this recorder with ${req.email}?`,
    note: "Only continue if this is your Notizli account: its recordings will go there.",
    ok: switching ? "Switch" : "Pair",
  });
  if (!ok) {
    await invoke("cancel_pair");
    return refresh();
  }
  try {
    const email = await invoke("confirm_pair");
    toast(`Paired with ${email}.`);
    mode = null;
  } catch (e) {
    toast(errText(e));
  }
  refresh();
}

async function submitToken() {
  const text = $("token-input").value;
  const msg = $("token-msg");
  msg.hidden = true;
  if (!text.trim()) return;
  $("submit-token-btn").disabled = true;
  try {
    const req = await invoke("submit_pair_text", { text });
    $("token-input").value = "";
    await askPair(req);
  } catch (e) {
    msg.textContent = errText(e);
    msg.hidden = false;
  } finally {
    $("submit-token-btn").disabled = false;
  }
}

// ---- recording -----------------------------------------------------------------

async function startRecording() {
  mode = "starting";
  $("starting-text").textContent =
    S.os === "macos"
      ? "Opening the microphone and the Mac's sound. If macOS asks, click Allow for the Microphone and System Audio Recording."
      : "Opening the microphone and the speaker your call plays on…";
  render();
  try {
    await invoke("start_recording", { title: $("meeting-name").value || null });
    mode = null;
    $("rec-status").textContent = "Saved as you go. Uploads when you finish.";
  } catch (e) {
    mode = {
      error: {
        title: "Recording didn't start",
        message: errText(e),
        note:
          S.os === "macos"
            ? "Check System Settings → Privacy & Security → Microphone (and System Audio Recording) for Notizli."
            : "Check Windows Settings → Privacy & security → Microphone, including \"Let desktop apps access your microphone\".",
      },
    };
  }
  refresh();
}

async function finishRecording() {
  $("stop-btn").disabled = true;
  mode = { uploading: S.recording.id };
  $("upload-label").textContent = "Saving…";
  $("upload-title").textContent = S.recording.title;
  $("upload-fill").style.width = "0%";
  show("s-uploading");
  try {
    const r = await invoke("finish_recording");
    $("meeting-name").value = "";
    if (r.saved_elsewhere) {
      mode = {
        error: {
          title: "Saved outside Notizli's folder",
          message: `The disk failed during the recording, so it was saved here instead: ${r.saved_elsewhere}`,
          note: "Upload it on notizli.ch, or keep the file.",
        },
      };
    } else if (!r.paired) {
      mode = { done: { meeting_id: null } };
    } else {
      mode = { uploading: r.id };
    }
  } catch (e) {
    mode = { error: { title: "The recording could not be finished", message: errText(e) } };
  } finally {
    $("stop-btn").disabled = false;
  }
  await refresh();
  settleUpload();
}

// The upload may already have finished while the window was busy.
function settleUpload() {
  if (!mode || !mode.uploading || !S) return;
  const id = mode.uploading;
  if (S.last_done && S.last_done.id === id) {
    mode = { done: S.last_done };
    return render();
  }
  const entry = S.unsent.find((x) => x.id === id);
  const busy = S.upload && S.upload.id === id;
  if (entry && !busy && entry.last_error) {
    mode = { error: uploadErrorView(id, entry.status === "too_large" ? "too_large" : "retry", entry.last_error) };
    render();
  }
}

function uploadErrorView(id, kind, message) {
  if (kind === "unauthorized") {
    return { title: "This recorder is no longer paired", message, note: "The recording is kept and uploads after you pair again.", pair: true, saveId: id };
  }
  if (kind === "too_large") {
    return { title: "The recording is too large", message, note: "Keep it with Save a copy…", saveId: id };
  }
  return { title: "Upload didn't go through", message, note: "The recording is kept and will be sent again automatically.", retryId: id, saveId: id };
}

async function discardRecording() {
  const ok = await confirmDialog({
    title: "Discard this recording?",
    text: "It will be deleted and not uploaded.",
    ok: "Discard",
    cancel: "Keep recording",
  });
  if (!ok) return;
  try {
    await invoke("discard_recording");
  } catch (e) {
    toast(errText(e));
  }
  mode = null;
  refresh();
}

// ---- events from Rust -------------------------------------------------------------

listen("rec", ({ payload: e }) => {
  switch (e.type) {
    case "levels": {
      const m = level(e.mic_peak);
      $("mic-fill").style.width = `${m}%`;
      const o = e.other_peak == null ? 0 : level(e.other_peak);
      $("other-fill").style.width = `${o}%`;
      break;
    }
    case "hearing":
      $("other-name").textContent = `Hearing: ${e.label}`;
      break;
    case "microphone":
      $("mic-name").textContent = e.label;
      break;
    case "other_side_silent":
      $("silent-warning").hidden = !e.silent;
      break;
    case "notice":
      $("rec-status").textContent = e.text;
      toast(e.text);
      break;
    case "disk_error":
      $("rec-status").textContent = "Can't write to the disk: the recording continues in memory. Don't quit until it is saved.";
      $("rec-status").className = "warn-text";
      break;
  }
});

listen("upload", ({ payload: u }) => {
  if (S && (u.phase === "progress" || u.phase === "start")) {
    S.upload = { id: u.id, sent: u.sent || 0, total: u.total || 0 };
    if (mode && mode.uploading === u.id) renderUploading();
    return;
  }
  if (u.phase === "done") {
    if (mode && mode.uploading === u.id) mode = { done: u };
    else toast(`Uploaded: ${u.title}`);
  } else if (u.phase === "failed") {
    if (mode && mode.uploading === u.id) mode = { error: uploadErrorView(u.id, u.kind, u.error) };
    else if (u.kind === "unauthorized") toast("This recorder is no longer paired. Recordings are kept until you pair again.");
  }
  refreshSoon();
});

listen("pair-request", ({ payload }) => askPair(payload));
listen("notice", ({ payload }) => toast(payload.text));
listen("state", () => refreshSoon());
listen("confirm-quit", async () => {
  const ok = await confirmDialog({
    title: "A recording is running",
    text: "Stop it, save it and quit? It uploads the next time Notizli starts.",
    ok: "Save and quit",
    cancel: "Keep recording",
  });
  if (ok) invoke("quit_app").catch((e) => toast(errText(e)));
});

// ---- buttons ---------------------------------------------------------------------

$("pair-btn").onclick = () => invoke("open_pair_page").catch((e) => toast(errText(e)));
$("banner-pair-btn").onclick = () => {
  mode = null;
  if (S) S.paired = false;
  show("s-unpaired");
};
$("done-pair-btn").onclick = $("error-pair-btn").onclick = () => {
  mode = null;
  show("s-unpaired");
};
$("submit-token-btn").onclick = submitToken;
$("token-input").addEventListener("keydown", (e) => {
  if (e.key === "Enter") submitToken();
});
$("record-unpaired-btn").onclick = () => {
  mode = "recordUnpaired";
  render();
};
$("record-btn").onclick = startRecording;
$("stop-btn").onclick = finishRecording;
$("discard-btn").onclick = discardRecording;
$("record-another-btn").onclick = () => {
  mode = null;
  refresh();
};
$("error-back-btn").onclick = () => {
  mode = null;
  refresh();
};
$("device").onchange = () => invoke("set_mic", { device: $("device").value || null });
$("unpair-btn").onclick = async () => {
  const ok = await confirmDialog({
    title: "Unpair this recorder?",
    text: `It will stop uploading to ${S.email || "your account"}. Recordings not yet uploaded stay on this computer.`,
    ok: "Unpair",
  });
  if (ok) invoke("unpair").then(refresh).catch((e) => toast(errText(e)));
};
window.addEventListener("focus", () => {
  micsLoaded = false;
  refreshSoon();
});

// Start: show state, and a pairing request that arrived before the window loaded.
refresh().then(() => {
  if (S && S.pair_request) askPair(S.pair_request);
  if (S && S.token_error) toast(`Couldn't read the saved pairing: ${S.token_error}`);
});
