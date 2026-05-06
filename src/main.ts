import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import hljs from "highlight.js";

// ── Types ──
interface Message {
  id: string;
  role: "user" | "assistant";
  content: string;
  tools?: ToolExec[];
}

interface ToolExec {
  name: string;
  args: string;
  result?: string;
  status: "running" | "done";
}

// ── State ──
let messages: Message[] = [];
let isStreaming = false;
let streamBuffer = "";
let activeTool: ToolExec | null = null;
let toolHistory: ToolExec[] = [];
let theme: "dark" | "light" = (localStorage.getItem("jarvis-theme") as "dark" | "light") || "dark";

const uid = () => Date.now().toString(36) + Math.random().toString(36).slice(2, 6);

// ── Event Listeners from Rust ──
listen<string>("chat-token", (event) => {
  streamBuffer += event.payload;
  updateStreamingBubble();
});

listen<{name: string, args: string}>("tool-start", (event) => {
  activeTool = { name: event.payload.name, args: event.payload.args, status: "running" };
  toolHistory.push(activeTool);
  updateToolDisplay();
});

listen<{name: string, result: string}>("tool-result", (event) => {
  if (activeTool) {
    activeTool.result = event.payload.result;
    activeTool.status = "done";
    activeTool = null;
  }
  updateToolDisplay();
});

listen<string>("chat-done", (_event) => {
  // Finalize the streaming message
  if (streamBuffer) {
    messages.push({ id: uid(), role: "assistant", content: streamBuffer, tools: [...toolHistory] });
  }
  streamBuffer = "";
  isStreaming = false;
  activeTool = null;
  toolHistory = [];
  render();
});

// ── API ──
async function sendMessage(text: string) {
  if (!text.trim() || isStreaming) return;

  messages.push({ id: uid(), role: "user", content: text });
  isStreaming = true;
  streamBuffer = "";
  render();

  try {
    await invoke("send_message", { message: text });
  } catch (e) {
    messages.push({ id: uid(), role: "assistant", content: `Error: ${e}` });
    isStreaming = false;
    render();
  }
}

// ── Markdown Rendering ──
function md(text: string): string {
  let s = text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

  // Code blocks with syntax highlighting
  s = s.replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) => {
    let highlighted: string;
    try {
      highlighted = lang
        ? hljs.highlight(code.trim(), { language: lang }).value
        : hljs.highlightAuto(code.trim()).value;
    } catch {
      highlighted = code.trim();
    }
    const header = lang ? `<span class="code-lang">${lang}</span>` : "";
    return `<pre><code class="hljs">${header}${highlighted}</code></pre>`;
  });

  s = s.replace(/`([^`]+)`/g, "<code>$1</code>");
  s = s.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  s = s.replace(/(?<!\w)\*(.+?)\*(?!\w)/g, "<em>$1</em>");
  s = s.replace(/^### (.+)$/gm, '<h4>$1</h4>');
  s = s.replace(/^## (.+)$/gm, '<h3>$1</h3>');
  s = s.replace(/^# (.+)$/gm, '<h2>$1</h2>');
  s = s.replace(/^[-*] (.+)$/gm, "<li>$1</li>");
  s = s.replace(/^(\d+)\. (.+)$/gm, "<li>$1. $2</li>");
  s = s.replace(/\n/g, "<br>");
  return s;
}

// ── Streaming Bubble ──
function updateStreamingBubble() {
  const chatArea = document.getElementById("chat-area");
  if (!chatArea) return;

  let bubble = document.getElementById("streaming-bubble");
  if (!bubble) {
    bubble = document.createElement("div");
    bubble.id = "streaming-bubble";
    bubble.className = "message assistant";
    bubble.innerHTML = '<div class="message-role">JARVIS</div><div class="tool-area"></div><div class="message-body"></div>';
    chatArea.appendChild(bubble);
  }

  const body = bubble.querySelector(".message-body");
  if (body) body.innerHTML = md(streamBuffer);

  chatArea.scrollTop = chatArea.scrollHeight;
}

function updateToolDisplay() {
  const chatArea = document.getElementById("chat-area");
  if (!chatArea) return;

  let bubble = document.getElementById("streaming-bubble");
  if (!bubble) {
    bubble = document.createElement("div");
    bubble.id = "streaming-bubble";
    bubble.className = "message assistant";
    bubble.innerHTML = '<div class="message-role">JARVIS</div><div class="tool-area"></div><div class="message-body"></div>';
    chatArea.appendChild(bubble);
  }

  const toolArea = bubble.querySelector(".tool-area");
  if (toolArea) {
    toolArea.innerHTML = toolHistory.map(t => {
      const statusIcon = t.status === "running" ? '<span class="tool-spinner">⚙️</span>' : '✓';
      const result = t.result ? `<div class="tool-result">${t.result.replace(/</g, "&lt;").slice(0, 150)}</div>` : '';
      return `<div class="tool-exec">
        <div class="tool-header">${statusIcon} <span class="tool-name">${t.name}</span>(${t.args.replace(/</g, "&lt;")})</div>
        ${result}
      </div>`;
    }).join("");
  }

  chatArea.scrollTop = chatArea.scrollHeight;
}

// ── Render ──
function render() {
  const app = document.getElementById("app")!;

  const messagesHtml = messages.map((m) => {
    const cls = m.role === "user" ? "user" : "assistant";
    const label = m.role === "user" ? "YOU" : "JARVIS";
    const body = m.role === "user"
      ? m.content.replace(/</g, "&lt;").replace(/\n/g, "<br>")
      : md(m.content);
    const toolsHtml = m.tools?.length
      ? `<div class="tool-area">${m.tools.map(t => `<div class="tool-exec"><div class="tool-header">✓ <span class="tool-name">${t.name}</span>(${t.args.replace(/</g, "&lt;")})</div>${t.result ? `<div class="tool-result">${t.result.replace(/</g, "&lt;").slice(0, 150)}</div>` : ""}</div>`).join("")}</div>`
      : "";
    return `<div class="message ${cls}">
      <div class="message-role">${label}</div>
      ${toolsHtml}
      <div class="message-body">${body}</div>
    </div>`;
  }).join("");

  const thinkingHtml = isStreaming && !streamBuffer
    ? '<div class="message assistant thinking"><div class="message-role">JARVIS</div><div class="message-body"><span class="dots">...</span></div></div>'
    : "";

  app.innerHTML = `
    <div class="app-layout">
      <header class="topbar">
        <div class="topbar-title">JARVIS</div>
        <div class="topbar-actions">
          <button class="btn-icon" id="btn-clear" title="Clear history">Clear</button>
          <button class="btn-icon" id="btn-theme" title="Toggle theme">${theme === "dark" ? "☀️" : "🌙"}</button>
        </div>
      </header>
      <main class="chat-container">
        <div class="chat-area" id="chat-area">
          ${messages.length === 0 ? '<div class="welcome"><div class="welcome-title">JARVIS</div><div class="welcome-sub">What can I help you with?</div></div>' : ""}
          ${messagesHtml}
          ${thinkingHtml}
        </div>
      </main>
      <footer class="input-container">
        <div class="input-bar">
          <textarea id="chat-input" placeholder="Message JARVIS..." ${isStreaming ? "disabled" : ""}></textarea>
          <button class="btn-send" id="btn-send" ${isStreaming ? "disabled" : ""}>Send</button>
        </div>
      </footer>
    </div>
  `;

  // Scroll to bottom
  const chatArea = document.getElementById("chat-area");
  if (chatArea) chatArea.scrollTop = chatArea.scrollHeight;

  // Attach listeners
  attachListeners();
}

function attachListeners() {
  const input = document.getElementById("chat-input") as HTMLTextAreaElement;
  const sendBtn = document.getElementById("btn-send");
  const clearBtn = document.getElementById("btn-clear");
  const themeBtn = document.getElementById("btn-theme");

  sendBtn?.addEventListener("click", () => {
    if (input?.value.trim()) {
      sendMessage(input.value.trim());
      input.value = "";
    }
  });

  input?.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (input.value.trim()) {
        sendMessage(input.value.trim());
        input.value = "";
      }
    }
  });

  // Auto-resize textarea
  input?.addEventListener("input", () => {
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, 200) + "px";
  });

  clearBtn?.addEventListener("click", async () => {
    messages = [];
    await invoke("clear_history");
    render();
  });

  themeBtn?.addEventListener("click", () => {
    theme = theme === "dark" ? "light" : "dark";
    localStorage.setItem("jarvis-theme", theme);
    applyTheme();
    render();
  });

  // Focus input
  if (!isStreaming) input?.focus();
}

function applyTheme() {
  document.documentElement.setAttribute("data-theme", theme);
}

// ── Init ──
applyTheme();
render();
