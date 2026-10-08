// The "Record this call?" box. Rust decides what it shows; buttons call back.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
let tick = null;

function render(v) {
  clearInterval(tick);
  if (!v) return;
  if (v.mode === "ask") {
    $("title").textContent = v.title;
    $("question").textContent = "Record it?";
    $("yes").textContent = "Record";
    $("no").textContent = "Not now";
    $("yes").onclick = () => invoke("prompt_record");
    $("no").onclick = () => invoke("prompt_dismiss");
    return;
  }
  let left = v.seconds;
  const update = () => ($("question").textContent = `Finishing in ${left} ${left === 1 ? "second" : "seconds"}`);
  $("title").textContent = "The call ended.";
  update();
  tick = setInterval(() => {
    left = Math.max(0, left - 1);
    update();
  }, 1000);
  $("yes").textContent = "Finish now";
  $("no").textContent = "Keep recording";
  $("yes").onclick = () => invoke("prompt_finish_now");
  $("no").onclick = () => invoke("prompt_keep");
}

listen("prompt", ({ payload }) => render(payload));
invoke("prompt_view").then(render);
