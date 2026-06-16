// Nebula Island — a sleek desktop expert that can see your screen.
// A frameless, always-on-top capsule pinned to the top-center of the screen.
// It morphs minimal -> compact -> consult -> thinking -> answer and is instantly
// consultable: ask by voice or text and it looks at your screen right then and
// answers like an expert who's been watching the whole time.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./island.css";

type State =
  | "minimal"   // idle pill
  | "compact"   // brief glanceable line (alerts)
  | "consult"   // input open: ask a question
  | "thinking"  // looking at screen / answering
  | "answer";   // showing the expert's answer

const model = {
  state: "minimal" as State,
  compactLine: "",
  question: "",
  answer: "",
  status: "",
  draft: "",      // preserved input text across re-renders
};

const root = document.getElementById("island-root")!;
let collapseTimer: number | undefined;
let busy = false;

// States that own the cursor / shouldn't be interrupted by background events.
const isInteractive = (s: State) => s === "consult" || s === "thinking" || s === "answer";

// ── Rendering ────────────────────────────────────────────────────────────────

function render() {
  const wasConsult = root.querySelector(".island-capsule.consult");
  root.className = `island island--${model.state}`;

  switch (model.state) {
    case "minimal":
      root.innerHTML = `<div class="island-pill" title="Press Ctrl+Shift+Space to ask Nebula about your screen"></div>`;
      break;

    case "compact":
      root.innerHTML = `
        <div class="island-capsule compact">
          <span class="island-eye">✦</span>
          <span class="island-compact-text"></span>
        </div>`;
      (root.querySelector(".island-compact-text") as HTMLElement).textContent = model.compactLine;
      break;

    case "thinking":
      root.innerHTML = `
        <div class="island-capsule compact thinking-capsule">
          <span class="island-eye thinking">✦</span>
          <span class="island-compact-text"></span>
          <span class="island-dots"><i></i><i></i><i></i></span>
        </div>`;
      (root.querySelector(".island-compact-text") as HTMLElement).textContent = model.status || "looking…";
      break;

    case "consult": {
      // If we're already showing the consult capsule, only update the status
      // line — never rebuild the input (that would drop focus + typed text).
      const existingInput = root.querySelector(".island-input") as HTMLInputElement | null;
      if (wasConsult && existingInput) {
        updateStatusLine();
        return;
      }
      root.innerHTML = `
        <div class="island-capsule consult">
          <div class="island-input-row">
            <span class="island-eye ${busy ? "thinking" : ""}">✦</span>
            <input class="island-input" type="text" placeholder="Ask about your screen…"
                   autocomplete="off" spellcheck="false" />
            <button class="island-mic" title="Speak">🎙</button>
            <button class="island-send" title="Ask">↵</button>
          </div>
          <div class="island-status-slot"></div>`;
      bindConsult();
      updateStatusLine();
      break;
    }

    case "answer":
      root.innerHTML = `
        <div class="island-capsule expanded">
          <div class="island-head">
            <span class="island-eye">✦</span>
            <span class="island-title"></span>
            <button class="island-close" title="Dismiss (Esc)">×</button>
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
      root.querySelector(".island-close")?.addEventListener("click", collapse);
      root.querySelector('[data-act="followup"]')?.addEventListener("click", openConsult);
      root.querySelector('[data-act="open"]')?.addEventListener("click", () => {
        const full = model.question ? `${model.question}\n\n${model.answer}` : model.answer;
        invoke("send_message", { message: `Continuing from the island — ${full}` }).catch(() => {});
        invoke("focus_main_window").catch(() => {});
        collapse();
      });
      break;
  }
}

function updateStatusLine() {
  const slot = root.querySelector(".island-status-slot");
  if (!slot) return;
  slot.innerHTML = model.status
    ? `<div class="island-status">${escapeHtml(model.status)}</div>`
    : "";
}

function escapeHtml(s: string): string {
  const d = document.createElement("div");
  d.textContent = s;
  return d.innerHTML;
}

function bindConsult() {
  const input = root.querySelector(".island-input") as HTMLInputElement;
  const send = root.querySelector(".island-send");
  const mic = root.querySelector(".island-mic");

  input.value = model.draft;
  // Defer focus a tick so the window/elements are ready (avoids missed focus).
  requestAnimationFrame(() => input.focus());

  input.addEventListener("input", () => { model.draft = input.value; });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); ask(input.value); }
    else if (e.key === "Escape") { e.preventDefault(); collapse(); }
  });
  send?.addEventListener("click", () => ask(input.value));
  mic?.addEventListener("click", listenThenAsk);
}

// ── State transitions ────────────────────────────────────────────────────────

function setState(s: State) {
  model.state = s;
  // Interactive states receive clicks; idle states are click-through so the
  // overlay never blocks the apps beneath it.
  getCurrentWindow().setIgnoreCursorEvents(!isInteractive(s)).catch(() => {});
  render();
}

function setStatus(text: string) {
  model.status = text;
  // Update just the status line if we can, else a full render.
  if (model.state === "consult" && root.querySelector(".island-status-slot")) updateStatusLine();
  else render();
}

function openConsult() {
  clearTimeout(collapseTimer);
  model.status = "";
  model.draft = "";
  setState("consult");
}

function showCompact(line: string, holdMs = 4000) {
  // Don't stomp an active interaction with a passing alert.
  if (isInteractive(model.state)) return;
  clearTimeout(collapseTimer);
  model.compactLine = line;
  setState("compact");
  collapseTimer = window.setTimeout(collapse, holdMs);
}

function showAnswer(question: string, answer: string) {
  clearTimeout(collapseTimer);
  model.question = question;
  model.answer = answer;
  model.status = "";
  setState("answer");
  collapseTimer = window.setTimeout(collapse, 30000);
}

function collapse() {
  clearTimeout(collapseTimer);
  model.status = "";
  model.draft = "";
  busy = false;
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
    showAnswer(q, res?.answer?.trim() || "(no answer)");
  } catch (e) {
    showAnswer(q, typeof e === "string" ? e : "Couldn't answer — try again.");
  } finally {
    busy = false;
  }
}

// Voice: record one utterance, then ask it. Stays in consult so the user sees
// "listening…" and the captured text flow naturally.
async function listenThenAsk() {
  if (busy) return;
  busy = true;
  setStatus("listening…");
  try {
    const text = await invoke<string>("island_listen_once");
    busy = false;
    const t = (text || "").trim();
    if (t) {
      model.draft = t;
      ask(t);
    } else {
      setStatus("didn't catch that — try again");
    }
  } catch (e) {
    busy = false;
    setStatus(typeof e === "string" ? e : "mic error");
  }
}

// ── Backend events ───────────────────────────────────────────────────────────

// Proactive suggestion from the observer loop — chime in, but never interrupt
// an active consult/answer.
listen<{ message: string; context: string }>("proactive-suggestion", (e) => {
  if (isInteractive(model.state)) return;
  showAnswer("", e.payload.message);
});

// Hard system alert -> brief compact flash (ignored mid-interaction).
listen<{ type: string; message: string }>("system-alert", (e) => {
  showCompact("⚠ " + e.payload.message, 5000);
});

// Global hotkey -> open the consult input (toggle: closes if already open).
listen("island-trigger", () => {
  if (model.state === "consult") collapse();
  else if (!busy) openConsult();
});

// STT sidecar went live.
listen("island-listening", () => {
  if (busy && model.state === "consult") setStatus("listening…");
});

getCurrentWindow().show().catch(() => {});
render();
