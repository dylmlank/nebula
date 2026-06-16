// Nebula Island — a desktop Dynamic Island.
// A frameless, always-on-top capsule pinned to the top-center of the screen
// that morphs through Apple's four-state model: minimal -> compact -> expanded.
// It's the always-visible "face" of nebula: it chimes in with proactive
// suggestions and, on a manual hotkey/click, reads the screen and reacts.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./island.css";

type State = "minimal" | "compact" | "expanded" | "thinking";

interface IslandModel {
  state: State;
  compactLine: string;       // one-line glanceable text for compact state
  title: string;             // expanded: short heading
  body: string;              // expanded: the observation / suggestion
  actions: { label: string; send: string }[]; // expanded action buttons
}

const model: IslandModel = {
  state: "minimal",
  compactLine: "",
  title: "",
  body: "",
  actions: [],
};

const root = document.getElementById("island-root")!;
let collapseTimer: number | undefined;

// ── Rendering ──────────────────────────────────────────────────────────────

function render() {
  root.className = `island island--${model.state}`;

  if (model.state === "minimal") {
    root.innerHTML = `<div class="island-pill" title="Nebula — click to ask about your screen"></div>`;
  } else if (model.state === "thinking") {
    root.innerHTML = `
      <div class="island-capsule compact">
        <span class="island-eye thinking">✦</span>
        <span class="island-compact-text">looking…</span>
        <span class="island-dots"><i></i><i></i><i></i></span>
      </div>`;
  } else if (model.state === "compact") {
    root.innerHTML = `
      <div class="island-capsule compact">
        <span class="island-eye">✦</span>
        <span class="island-compact-text"></span>
      </div>`;
    (root.querySelector(".island-compact-text") as HTMLElement).textContent = model.compactLine;
  } else {
    // expanded
    const actions = model.actions
      .map((a, i) => `<button class="island-action" data-i="${i}">${escapeHtml(a.label)}</button>`)
      .join("");
    root.innerHTML = `
      <div class="island-capsule expanded">
        <div class="island-head">
          <span class="island-eye">✦</span>
          <span class="island-title"></span>
          <button class="island-close" title="Dismiss">×</button>
        </div>
        <div class="island-body"></div>
        <div class="island-actions">${actions}</div>
      </div>`;
    (root.querySelector(".island-title") as HTMLElement).textContent = model.title || "Nebula";
    (root.querySelector(".island-body") as HTMLElement).textContent = model.body;
    root.querySelector(".island-close")?.addEventListener("click", (e) => {
      e.stopPropagation();
      collapse();
    });
    root.querySelectorAll<HTMLButtonElement>(".island-action").forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const i = Number(btn.dataset.i);
        const act = model.actions[i];
        // Empty send = dismiss-only button ("Got it" / "Dismiss").
        if (act && act.send.trim() !== "") {
          invoke("send_message", { message: act.send }).catch(() => {});
          invoke("focus_main_window").catch(() => {});
        }
        collapse();
      });
    });
  }

  // Click anywhere on a non-expanded island = trigger a screen read.
  if (model.state === "minimal" || model.state === "compact") {
    root.querySelector(".island-pill, .island-capsule")?.addEventListener("click", triggerScreenRead);
  }
}

function escapeHtml(s: string): string {
  const d = document.createElement("div");
  d.textContent = s;
  return d.innerHTML;
}

// ── State transitions ────────────────────────────────────────────────────────

function setState(s: State) {
  model.state = s;
  render();
}

function expandWith(title: string, body: string, actions: { label: string; send: string }[]) {
  clearTimeout(collapseTimer);
  model.title = title;
  model.body = body;
  model.actions = actions.slice(0, 2); // glanceable: at most 2 controls
  setState("expanded");
  // Auto-collapse after a while so it never lingers / piles up.
  collapseTimer = window.setTimeout(collapse, 18000);
}

function showCompact(line: string, holdMs = 6000) {
  clearTimeout(collapseTimer);
  model.compactLine = line;
  setState("compact");
  collapseTimer = window.setTimeout(collapse, holdMs);
}

function collapse() {
  clearTimeout(collapseTimer);
  setState("minimal");
}

// ── Manual screen-read trigger (hotkey or click) ─────────────────────────────

let busy = false;
async function triggerScreenRead() {
  if (busy) return;
  busy = true;
  setState("thinking");
  try {
    // Backend: grim screenshot -> analyze_screen (vision) -> consider_proactive.
    // Returns { say: string|null }. If it has something to say, expand; else a
    // brief "nothing to add" compact, then collapse.
    const res = await invoke<{ say: string | null }>("island_observe_screen");
    if (res && res.say) {
      expandWith("Nebula", res.say, [
        { label: "Tell me more", send: `About what you just saw on my screen: ${res.say} — go deeper.` },
        { label: "Dismiss", send: "" },
      ]);
    } else {
      showCompact("nothing to add 👀", 2500);
    }
  } catch (_e) {
    showCompact("couldn't read screen", 2500);
  } finally {
    busy = false;
  }
}

// ── Wire backend events ──────────────────────────────────────────────────────

// Proactive suggestion from the observer loop (already emitted by main.rs).
listen<{ message: string; context: string }>("proactive-suggestion", (e) => {
  expandWith("Nebula", e.payload.message, [
    { label: "Act on it", send: e.payload.message },
    { label: "Got it", send: "" },
  ]);
});

// Hard system alert -> brief compact flash.
listen<{ type: string; message: string }>("system-alert", (e) => {
  showCompact("⚠ " + e.payload.message, 5000);
});

// Global hotkey fired in the backend -> trigger a screen read.
listen("island-trigger", () => { triggerScreenRead(); });

// Show the island once the window is ready.
getCurrentWindow().show().catch(() => {});
render();
