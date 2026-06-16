// Nebula Island — a sleek desktop expert that can see your screen.
// A frameless, always-on-top capsule pinned to the top-center of the screen.
// It morphs minimal -> compact -> expanded (Apple Dynamic Island model) and is
// instantly consultable: ask by voice or text and it looks at your screen right
// then and answers like an expert who's been watching the whole time.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./island.css";

type State =
  | "minimal"      // idle pill
  | "compact"      // brief glanceable line
  | "consult"      // input open: ask a question
  | "thinking"     // looking at screen / answering
  | "answer";      // showing the expert's answer

interface Model {
  state: State;
  compactLine: string;
  question: string;   // what was asked (shown above the answer)
  answer: string;
  status: string;     // transient line under input ("listening…", etc.)
}

const model: Model = {
  state: "minimal",
  compactLine: "",
  question: "",
  answer: "",
  status: "",
};

const root = document.getElementById("island-root")!;
let collapseTimer: number | undefined;
let busy = false;

// ── Rendering ────────────────────────────────────────────────────────────────

function render() {
  root.className = `island island--${model.state}`;

  switch (model.state) {
    case "minimal":
      root.innerHTML = `<div class="island-pill" title="Press Ctrl+Shift+Space to ask Nebula about your screen"></div>`;
      bindOpen(root.querySelector(".island-pill"));
      break;

    case "compact":
      root.innerHTML = `
        <div class="island-capsule compact">
          <span class="island-eye">✦</span>
          <span class="island-compact-text"></span>
        </div>`;
      (root.querySelector(".island-compact-text") as HTMLElement).textContent = model.compactLine;
      bindOpen(root.querySelector(".island-capsule"));
      break;

    case "thinking":
      root.innerHTML = `
        <div class="island-capsule compact">
          <span class="island-eye thinking">✦</span>
          <span class="island-compact-text">${escapeHtml(model.status || "looking…")}</span>
          <span class="island-dots"><i></i><i></i><i></i></span>
        </div>`;
      break;

    case "consult":
      root.innerHTML = `
        <div class="island-capsule consult">
          <div class="island-input-row">
            <span class="island-eye">✦</span>
            <input class="island-input" type="text" placeholder="Ask about your screen…" autocomplete="off" />
            <button class="island-mic" title="Speak (or press the hotkey)">🎙</button>
            <button class="island-send" title="Ask">↵</button>
          </div>
          ${model.status ? `<div class="island-status">${escapeHtml(model.status)}</div>` : ""}
        </div>`;
      bindConsult();
      break;

    case "answer":
      root.innerHTML = `
        <div class="island-capsule expanded">
          <div class="island-head">
            <span class="island-eye">✦</span>
            <span class="island-title"></span>
            <button class="island-close" title="Dismiss">×</button>
          </div>
          <div class="island-answer"></div>
          <div class="island-actions">
            <button class="island-action" data-act="followup">Ask follow-up</button>
            <button class="island-action" data-act="open">Open in chat</button>
          </div>
        </div>`;
      (root.querySelector(".island-title") as HTMLElement).textContent =
        model.question ? `“${model.question}”` : "Nebula";
      (root.querySelector(".island-answer") as HTMLElement).textContent = model.answer;
      root.querySelector(".island-close")?.addEventListener("click", (e) => { e.stopPropagation(); collapse(); });
      root.querySelector('[data-act="followup"]')?.addEventListener("click", (e) => { e.stopPropagation(); openConsult(); });
      root.querySelector('[data-act="open"]')?.addEventListener("click", (e) => {
        e.stopPropagation();
        const full = model.question ? `${model.question}\n\n${model.answer}` : model.answer;
        invoke("send_message", { message: `Continuing from the island — ${full}` }).catch(() => {});
        invoke("focus_main_window").catch(() => {});
        collapse();
      });
      break;
  }
}

function escapeHtml(s: string): string {
  const d = document.createElement("div");
  d.textContent = s;
  return d.innerHTML;
}

function bindOpen(el: Element | null) {
  el?.addEventListener("click", () => openConsult());
}

function bindConsult() {
  const input = root.querySelector(".island-input") as HTMLInputElement | null;
  const send = root.querySelector(".island-send");
  const mic = root.querySelector(".island-mic");

  input?.focus();
  input?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); ask(input.value); }
    if (e.key === "Escape") { collapse(); }
  });
  send?.addEventListener("click", () => ask(input?.value || ""));
  mic?.addEventListener("click", () => listenThenAsk());
}

// ── State transitions ────────────────────────────────────────────────────────

function setState(s: State) {
  model.state = s;
  // Interactive states need to receive clicks; idle states are click-through so
  // the overlay never blocks the apps beneath it.
  const interactive = s === "consult" || s === "answer" || s === "thinking";
  getCurrentWindow().setIgnoreCursorEvents(!interactive).catch(() => {});
  render();
}

function openConsult() {
  clearTimeout(collapseTimer);
  model.status = "";
  setState("consult");
}

function showCompact(line: string, holdMs = 4000) {
  clearTimeout(collapseTimer);
  model.compactLine = line;
  setState("compact");
  collapseTimer = window.setTimeout(collapse, holdMs);
}

function showAnswer(question: string, answer: string) {
  clearTimeout(collapseTimer);
  model.question = question;
  model.answer = answer;
  setState("answer");
  // Let the user read; auto-collapse only after a long while.
  collapseTimer = window.setTimeout(collapse, 30000);
}

function collapse() {
  clearTimeout(collapseTimer);
  model.status = "";
  setState("minimal");
}

// ── Core: ask the expert (captures screen + answers) ─────────────────────────

async function ask(question: string) {
  const q = question.trim();
  if (!q || busy) return;
  busy = true;
  model.status = "looking at your screen…";
  setState("thinking");
  try {
    const res = await invoke<{ answer: string }>("island_consult", { question: q });
    showAnswer(q, res?.answer || "(no answer)");
  } catch (e) {
    showAnswer(q, typeof e === "string" ? e : "Couldn't answer — try again.");
  } finally {
    busy = false;
  }
}

// Voice: record one utterance, then ask it.
async function listenThenAsk() {
  if (busy) return;
  busy = true;
  model.status = "listening…";
  setState("consult");
  render(); // ensure status shows
  try {
    const text = await invoke<string>("island_listen_once");
    busy = false;
    if (text && text.trim()) {
      ask(text);
    } else {
      model.status = "didn't catch that — try again";
      setState("consult");
    }
  } catch (e) {
    busy = false;
    model.status = typeof e === "string" ? e : "mic error";
    setState("consult");
  }
}

// ── Backend events ───────────────────────────────────────────────────────────

// Proactive suggestion from the observer loop — chime in as an answer card.
listen<{ message: string; context: string }>("proactive-suggestion", (e) => {
  showAnswer("", e.payload.message);
});

// Hard system alert -> brief compact flash.
listen<{ type: string; message: string }>("system-alert", (e) => {
  showCompact("⚠ " + e.payload.message, 5000);
});

// Global hotkey -> open the consult input (ready for voice or text).
listen("island-trigger", () => { openConsult(); });

// STT sidecar went live.
listen("island-listening", () => {
  if (model.state === "consult") { model.status = "listening…"; render(); }
});

getCurrentWindow().show().catch(() => {});
render();
