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

interface AppConfig {
  api_key: string;
  base_url: string;
  model: string;
  fast_model: string;
  fallback_models: string[];
  max_tokens: number;
  fast_max_tokens: number;
  vault_path: string;
  voice: { enabled: boolean; model: string; speed: number };
  theme: string;
}

interface VaultResult {
  path: string;
  header: string;
  text: string;
  score: number;
}

// ── State ──
let messages: Message[] = [];
let isStreaming = false;
let streamBuffer = "";
let activeTool: ToolExec | null = null;
let toolHistory: ToolExec[] = [];
let theme: "dark" | "light" = (localStorage.getItem("jarvis-theme") as "dark" | "light") || "dark";
let currentPanel: "chat" | "vault" | "settings" = "chat";
let vaultResults: VaultResult[] = [];
let vaultQuery = "";

const uid = () => Date.now().toString(36) + Math.random().toString(36).slice(2, 6);

// ── Event Listeners from Rust ──
listen<string>("chat-token", (event) => {
  if (!streamBuffer) console.log("[jarvis] First token received");
  streamBuffer += event.payload;
  updateStreamingBubble();
});

listen<{name: string, args: string}>("tool-start", (event) => {
  console.log("[jarvis] Tool start:", event.payload.name);
  activeTool = { name: event.payload.name, args: event.payload.args, status: "running" };
  toolHistory.push(activeTool);
  updateToolDisplay();
});

listen<{name: string, result: string}>("tool-result", (event) => {
  console.log("[jarvis] Tool result:", event.payload.name);
  if (activeTool) {
    activeTool.result = event.payload.result;
    activeTool.status = "done";
    activeTool = null;
  }
  updateToolDisplay();
});

listen<string>("chat-done", (_event) => {
  console.log("[jarvis] Chat done, buffer length:", streamBuffer.length);
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
  currentPanel = "chat";
  render();
  try {
    console.log("[jarvis] Sending message:", text.slice(0, 50));
    const result = await invoke("send_message", { message: text });
    console.log("[jarvis] invoke result:", result);
  } catch (e) {
    console.error("[jarvis] invoke error:", e);
    messages.push({ id: uid(), role: "assistant", content: `Error: ${e}` });
    isStreaming = false;
    render();
  }
}

async function searchVault(query: string) {
  if (!query.trim()) return;
  vaultQuery = query;
  try {
    vaultResults = await invoke<VaultResult[]>("search_vault", { query });
  } catch {
    vaultResults = [];
  }
  render();
}

// ── Markdown Rendering ──
function md(text: string): string {
  let s = text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  s = s.replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) => {
    let highlighted: string;
    try {
      highlighted = lang ? hljs.highlight(code.trim(), { language: lang }).value : hljs.highlightAuto(code.trim()).value;
    } catch { highlighted = code.trim(); }
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
      return `<div class="tool-exec"><div class="tool-header">${statusIcon} <span class="tool-name">${t.name}</span>(${t.args.replace(/</g, "&lt;")})</div>${result}</div>`;
    }).join("");
  }
  chatArea.scrollTop = chatArea.scrollHeight;
}

// ── Render ──
function render() {
  const app = document.getElementById("app")!;

  app.innerHTML = `
    <div class="app-layout">
      <header class="topbar">
        <div class="topbar-left">
          <div class="topbar-title">JARVIS</div>
          <nav class="topbar-nav">
            <button class="nav-tab ${currentPanel === "chat" ? "active" : ""}" data-panel="chat">Chat</button>
            <button class="nav-tab ${currentPanel === "vault" ? "active" : ""}" data-panel="vault">Vault</button>
            <button class="nav-tab ${currentPanel === "settings" ? "active" : ""}" data-panel="settings">Settings</button>
          </nav>
        </div>
        <div class="topbar-actions">
          <button class="btn-icon" id="btn-voice" title="Toggle voice">🔊</button>
          <button class="btn-icon" id="btn-clear" title="Clear history">Clear</button>
          <button class="btn-icon" id="btn-theme" title="Toggle theme">${theme === "dark" ? "☀️" : "🌙"}</button>
        </div>
      </header>
      <main class="panel-container">
        ${currentPanel === "chat" ? renderChat() : ""}
        ${currentPanel === "vault" ? renderVault() : ""}
        ${currentPanel === "settings" ? renderSettings() : ""}
      </main>
      ${currentPanel === "chat" ? `<footer class="input-container"><div class="input-bar"><textarea id="chat-input" placeholder="Message JARVIS..." ${isStreaming ? "disabled" : ""}></textarea><button class="btn-send" id="btn-send" ${isStreaming ? "disabled" : ""}>Send</button></div></footer>` : ""}
    </div>
  `;

  const chatArea = document.getElementById("chat-area");
  if (chatArea) chatArea.scrollTop = chatArea.scrollHeight;
  attachListeners();
}

function renderChat(): string {
  const messagesHtml = messages.map((m) => {
    const cls = m.role === "user" ? "user" : "assistant";
    const label = m.role === "user" ? "YOU" : "JARVIS";
    const body = m.role === "user"
      ? m.content.replace(/</g, "&lt;").replace(/\n/g, "<br>")
      : md(m.content);
    const toolsHtml = m.tools?.length
      ? `<div class="tool-area">${m.tools.map(t => `<div class="tool-exec"><div class="tool-header">✓ <span class="tool-name">${t.name}</span>(${t.args.replace(/</g, "&lt;")})</div>${t.result ? `<div class="tool-result">${t.result.replace(/</g, "&lt;").slice(0, 150)}</div>` : ""}</div>`).join("")}</div>`
      : "";
    return `<div class="message ${cls}"><div class="message-role">${label}</div>${toolsHtml}<div class="message-body">${body}</div></div>`;
  }).join("");

  const thinkingHtml = isStreaming && !streamBuffer
    ? '<div class="message assistant thinking"><div class="message-role">JARVIS</div><div class="message-body"><span class="dots">...</span></div></div>'
    : "";

  return `<div class="chat-area" id="chat-area">
    ${messages.length === 0 ? '<div class="welcome"><div class="welcome-title">JARVIS</div><div class="welcome-sub">What can I help you with?</div></div>' : ""}
    ${messagesHtml}${thinkingHtml}
  </div>`;
}

function renderVault(): string {
  const resultsHtml = vaultResults.map(r => `
    <div class="vault-result">
      <div class="vault-result-header">
        <span class="vault-path">${r.path}</span>
        <span class="vault-score">${(r.score * 100).toFixed(0)}%</span>
      </div>
      <div class="vault-result-section">${r.header}</div>
      <div class="vault-result-snippet">${r.text.replace(/</g, "&lt;").slice(0, 200)}</div>
      <button class="btn-ask-vault" data-vault-text="${encodeURIComponent(r.text.slice(0, 500))}">Ask about this</button>
    </div>
  `).join("");

  return `<div class="vault-panel">
    <div class="vault-search-bar">
      <input type="text" id="vault-search-input" class="vault-input" placeholder="Search your vault..." value="${vaultQuery}">
      <button class="btn-send" id="btn-vault-search">Search</button>
      <button class="btn-icon" id="btn-vault-reindex" title="Re-index vault">🔄</button>
    </div>
    <div class="vault-results">
      ${vaultResults.length === 0 && vaultQuery ? '<div class="vault-empty">No results found.</div>' : ""}
      ${!vaultQuery ? '<div class="vault-empty">Search your Obsidian vault for notes, decisions, ideas, and more.</div>' : ""}
      ${resultsHtml}
    </div>
  </div>`;
}

function renderSettings(): string {
  return `<div class="settings-panel" id="settings-panel">
    <h3 class="settings-title">Settings</h3>
    <div class="settings-group">
      <label class="settings-label">API Key</label>
      <input type="password" id="set-api-key" class="settings-input" placeholder="sk-...">
    </div>
    <div class="settings-group">
      <label class="settings-label">Base URL</label>
      <input type="text" id="set-base-url" class="settings-input" placeholder="https://proxy.shift.aws.dev">
    </div>
    <div class="settings-group">
      <label class="settings-label">Model</label>
      <input type="text" id="set-model" class="settings-input" placeholder="us.anthropic.claude-sonnet-4-6">
    </div>
    <div class="settings-group">
      <label class="settings-label">Fast Model (tool followups)</label>
      <input type="text" id="set-fast-model" class="settings-input" placeholder="anthropic.claude-haiku-4-5-20251001-v1:0">
    </div>
    <div class="settings-group">
      <label class="settings-label">Max Tokens</label>
      <input type="number" id="set-max-tokens" class="settings-input" placeholder="4096">
    </div>
    <div class="settings-group">
      <label class="settings-label">Vault Path</label>
      <input type="text" id="set-vault-path" class="settings-input" placeholder="~/Documents/vault/JARVIS">
    </div>
    <div class="settings-group">
      <label class="settings-label">Voice Model Path</label>
      <input type="text" id="set-voice-model" class="settings-input" placeholder="~/.local/share/piper-voices/en_GB-alan-medium.onnx">
    </div>
    <div class="settings-group">
      <label class="settings-label">Voice Speed</label>
      <input type="number" id="set-voice-speed" class="settings-input" step="0.1" min="0.5" max="2.0" placeholder="1.1">
    </div>
    <div class="settings-actions">
      <button class="btn-send" id="btn-save-settings">Save Settings</button>
      <span class="settings-status" id="settings-status"></span>
    </div>
  </div>`;
}

// ── Listeners ──
function attachListeners() {
  // Navigation
  document.querySelectorAll("[data-panel]").forEach(el => {
    el.addEventListener("click", () => {
      currentPanel = (el as HTMLElement).dataset.panel as typeof currentPanel;
      render();
      // Load settings when switching to settings panel
      if (currentPanel === "settings") loadSettings();
    });
  });

  // Chat
  const input = document.getElementById("chat-input") as HTMLTextAreaElement;
  const sendBtn = document.getElementById("btn-send");
  document.getElementById("btn-send")?.addEventListener("click", () => {
    if (input?.value.trim()) { sendMessage(input.value.trim()); input.value = ""; }
  });
  input?.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (input.value.trim()) { sendMessage(input.value.trim()); input.value = ""; }
    }
  });
  input?.addEventListener("input", () => {
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, 200) + "px";
  });

  // Voice
  document.getElementById("btn-voice")?.addEventListener("click", async () => {
    const enabled = await invoke<boolean>("toggle_voice");
    const btn = document.getElementById("btn-voice");
    if (btn) btn.textContent = enabled ? "🔊" : "🔇";
  });

  // Clear
  document.getElementById("btn-clear")?.addEventListener("click", async () => {
    messages = [];
    await invoke("clear_history");
    render();
  });

  // Theme
  document.getElementById("btn-theme")?.addEventListener("click", () => {
    theme = theme === "dark" ? "light" : "dark";
    localStorage.setItem("jarvis-theme", theme);
    applyTheme();
    render();
  });

  // Vault search
  const vaultInput = document.getElementById("vault-search-input") as HTMLInputElement;
  document.getElementById("btn-vault-search")?.addEventListener("click", () => {
    if (vaultInput?.value.trim()) searchVault(vaultInput.value.trim());
  });
  vaultInput?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); if (vaultInput.value.trim()) searchVault(vaultInput.value.trim()); }
  });
  document.getElementById("btn-vault-reindex")?.addEventListener("click", async () => {
    const btn = document.getElementById("btn-vault-reindex");
    if (btn) btn.textContent = "⏳";
    try {
      await invoke("reindex_vault", { force: true });
      if (btn) btn.textContent = "✓";
      setTimeout(() => { if (btn) btn.textContent = "🔄"; }, 2000);
    } catch {
      if (btn) btn.textContent = "✗";
    }
  });

  // Ask about vault result
  document.querySelectorAll("[data-vault-text]").forEach(el => {
    el.addEventListener("click", () => {
      const text = decodeURIComponent((el as HTMLElement).dataset.vaultText || "");
      currentPanel = "chat";
      render();
      sendMessage(`Based on this note from my vault:\n\n${text}\n\nSummarize the key points.`);
    });
  });

  // Settings save
  document.getElementById("btn-save-settings")?.addEventListener("click", saveSettings);

  // Focus input
  if (currentPanel === "chat" && !isStreaming) input?.focus();
  if (currentPanel === "vault") vaultInput?.focus();

  // Global shortcuts
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey && e.key === "l") { e.preventDefault(); messages = []; invoke("clear_history"); render(); }
    if (e.ctrlKey && e.key === "k") { e.preventDefault(); currentPanel = "vault"; render(); }
  });
}

// ── Settings ──
async function loadSettings() {
  try {
    const config = await invoke<AppConfig>("get_config");
    const setVal = (id: string, val: string) => {
      const el = document.getElementById(id) as HTMLInputElement;
      if (el) el.value = val;
    };
    setVal("set-api-key", config.api_key);
    setVal("set-base-url", config.base_url);
    setVal("set-model", config.model);
    setVal("set-fast-model", config.fast_model);
    setVal("set-max-tokens", String(config.max_tokens));
    setVal("set-vault-path", config.vault_path);
    setVal("set-voice-model", config.voice.model);
    setVal("set-voice-speed", String(config.voice.speed));
  } catch {}
}

async function saveSettings() {
  const getVal = (id: string) => (document.getElementById(id) as HTMLInputElement)?.value || "";
  const config: AppConfig = {
    api_key: getVal("set-api-key"),
    base_url: getVal("set-base-url"),
    model: getVal("set-model"),
    fast_model: getVal("set-fast-model"),
    fallback_models: [
      "anthropic.claude-sonnet-4-6",
      "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
    ],
    max_tokens: parseInt(getVal("set-max-tokens")) || 4096,
    fast_max_tokens: 2048,
    vault_path: getVal("set-vault-path"),
    voice: {
      enabled: true,
      model: getVal("set-voice-model"),
      speed: parseFloat(getVal("set-voice-speed")) || 1.1,
    },
    theme,
  };
  try {
    await invoke("save_config", { config });
    const status = document.getElementById("settings-status");
    if (status) { status.textContent = "Saved ✓"; setTimeout(() => { status.textContent = ""; }, 2000); }
  } catch (e) {
    const status = document.getElementById("settings-status");
    if (status) status.textContent = `Error: ${e}`;
  }
}

function applyTheme() {
  document.documentElement.setAttribute("data-theme", theme);
}

// ── Init ──
applyTheme();
render();

// Test IPC on startup
(async () => {
  try {
    const result = await invoke<string>("ping");
    console.log("[jarvis] IPC working:", result);
  } catch (e) {
    console.error("[jarvis] IPC FAILED:", e);
    // Show error in UI
    const app = document.getElementById("app");
    if (app) app.innerHTML += `<div style="position:fixed;top:0;left:0;right:0;padding:10px;background:red;color:white;z-index:9999">IPC Error: ${e}</div>`;
  }
})();
