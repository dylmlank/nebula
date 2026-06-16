import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import hljs from "highlight.js";
import "./style.css";

// ── Types ──
interface Message { id: string; role: "user" | "assistant"; content: string; tools?: ToolExec[]; }
interface ToolExec { name: string; args: string; result?: string; status: "running" | "done"; }
interface Conversation { id: string; title: string; messages: Message[]; created: string; updated: string; }
interface VaultResult { path: string; header: string; text: string; score: number; }
interface AppConfig { api_key: string; base_url: string; model: string; fast_model: string; fallback_models: string[]; max_tokens: number; fast_max_tokens: number; vault_path: string; voice: { enabled: boolean; model: string; speed: number }; theme: string; google: { search_api_key: string; search_cx: string; client_id: string; client_secret: string }; }

// ── State ──
let messages: Message[] = [];
let isStreaming = false;
let streamBuffer = "";
let activeTool: ToolExec | null = null;
let toolHistory: ToolExec[] = [];
let theme: "dark" | "light" = (localStorage.getItem("jarvis-theme") as "dark" | "light") || "dark";
let currentPanel: "chat" | "vault" | "settings" | "voice" = "chat";
let vaultResults: VaultResult[] = [];
let vaultQuery = "";
let conversations: Conversation[] = [];
let voiceModeActive = false;
let voiceState: "idle" | "listening" | "wake" | "processing" = "idle";
let voiceTranscript = "";
let voiceInitiatedMessage = false; // tracks if current message was from voice
let currentConvoId = "";
let sidebarOpen = localStorage.getItem("jarvis-sidebar") !== "closed";

// ── Attachments ──
interface Attachment {
  name: string;
  type: "image" | "document";
  data: string; // base64 for images, extracted text for docs
  mime?: string;
  preview?: string; // data URL for image thumbnails
}
let pendingAttachments: Attachment[] = [];

const uid = () => Date.now().toString(36) + Math.random().toString(36).slice(2, 6);

// ── Events ──
let tokenBatchTimer: number | null = null;
listen<string>("chat-token", (e) => {
  streamBuffer += e.payload;
  // Batch token updates — only render every 30ms for smoothness
  if (!tokenBatchTimer) {
    tokenBatchTimer = window.setTimeout(() => {
      updateStreamingBubble();
      tokenBatchTimer = null;
    }, 30);
  }
});
listen<{name: string, args: string}>("tool-start", (e) => { activeTool = { name: e.payload.name, args: e.payload.args, status: "running" }; toolHistory.push(activeTool); updateToolDisplay(); });
listen<{name: string, result: string}>("tool-result", (e) => { if (activeTool) { activeTool.result = e.payload.result; activeTool.status = "done"; activeTool = null; } updateToolDisplay(); });
listen<string>("chat-done", () => {
  if (streamBuffer) {
    if (voiceInitiatedMessage) {
      // Voice mode: speak the response, don't add to chat messages
      invoke("speak_response", { text: streamBuffer }).catch(() => {});
      voiceTranscript = "";
      voiceState = "idle";
      voiceInitiatedMessage = false;
      streamBuffer = "";
      isStreaming = false;
      activeTool = null;
      toolHistory = [];
      render();
    } else {
      // Chat mode: add to messages as normal
      messages.push({ id: uid(), role: "assistant", content: streamBuffer, tools: [...toolHistory] });
      streamBuffer = "";
      isStreaming = false;
      activeTool = null;
      toolHistory = [];
      saveCurrentConvo();
      render();
    }
  } else {
    streamBuffer = ""; isStreaming = false; activeTool = null; toolHistory = []; voiceInitiatedMessage = false; render();
  }
});

// System alerts from proactive monitor
listen<{type: string, message: string}>("system-alert", (e) => {
  const alert = e.payload;
  // Show as a notification toast
  const toast = document.createElement("div");
  toast.className = "toast-alert";
  toast.innerHTML = `<span class="toast-icon">⚠</span> ${alert.message}`;
  document.body.appendChild(toast);
  setTimeout(() => toast.classList.add("show"), 10);
  setTimeout(() => { toast.classList.remove("show"); setTimeout(() => toast.remove(), 300); }, 5000);
});

// Voice mode events from Python sidecar
listen<{type: string, text?: string, message?: string}>("voice-mode-event", (e) => {
  const ev = e.payload;
  if (ev.type === "ready") { voiceState = "idle"; voiceModeActive = true; voiceTranscript = ""; render(); }
  else if (ev.type === "wake") { voiceState = "wake"; render(); }
  else if (ev.type === "listening") { voiceState = "listening"; render(); }
  else if (ev.type === "interrupt") {
    // User spoke while JARVIS was talking — kill TTS
    invoke("stop_speaking").catch(() => {});
    voiceTranscript = "Interrupted — listening...";
    render();
  }
  else if (ev.type === "resume") {
    // User said resume/continue — do nothing special, just acknowledge
    voiceTranscript = "";
    voiceState = "wake";
    render();
  }
  else if (ev.type === "transcript" && ev.text) {
    voiceTranscript = ev.text;
    voiceState = "processing";
    render();
    voiceChat(ev.text);
  }
  else if (ev.type === "stopped") { voiceModeActive = false; voiceState = "idle"; render(); }
  else if (ev.type === "status" && ev.message) { voiceTranscript = ev.message; render(); }
  else if (ev.type === "error" && ev.message) {
    voiceTranscript = `Error: ${ev.message}`;
    render();
  }
});

// ── API ──
async function sendMessage(text: string) {
  if (!text.trim() || isStreaming) return;

  // Build message with attachments
  let displayText = text;
  let fullText = text;
  const imageAttachments: {mime_type: string, data: string}[] = [];

  if (pendingAttachments.length > 0) {
    const docTexts: string[] = [];
    for (const att of pendingAttachments) {
      if (att.type === "image" && att.mime && att.data) {
        imageAttachments.push({ mime_type: att.mime, data: att.data });
        displayText = `[📎 ${att.name}] ${text}`;
      } else if (att.type === "document") {
        docTexts.push(`[Attached: ${att.name}]\n${att.data}`);
        displayText = `[📎 ${att.name}] ${text}`;
      }
    }
    if (docTexts.length > 0) {
      fullText = docTexts.join("\n\n") + "\n\n" + text;
    }
    pendingAttachments = [];
  }

  messages.push({ id: uid(), role: "user", content: displayText });
  isStreaming = true; streamBuffer = ""; currentPanel = "chat"; render();

  try {
    if (imageAttachments.length > 0) {
      await invoke("send_message_with_images", { message: fullText, images: imageAttachments });
    } else {
      await invoke("send_message", { message: fullText });
    }
  } catch (e) { messages.push({ id: uid(), role: "assistant", content: `Error: ${e}` }); isStreaming = false; render(); }
}

async function searchVault(query: string) {
  if (!query.trim()) return;
  vaultQuery = query;
  try { vaultResults = await invoke<VaultResult[]>("search_vault", { query }); } catch { vaultResults = []; }
  render();
}

// ── File Upload ──
async function handleFileUpload(files: FileList) {
  for (const file of Array.from(files)) {
    const reader = new FileReader();
    reader.onload = async () => {
      const arrayBuffer = reader.result as ArrayBuffer;
      const bytes = new Uint8Array(arrayBuffer);
      let binary = '';
      for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
      const b64 = btoa(binary);

      const ext = file.name.split('.').pop()?.toLowerCase() || '';
      const imageExts = ['png', 'jpg', 'jpeg', 'gif', 'webp'];

      if (imageExts.includes(ext)) {
        // Image: store for vision API
        const mime = file.type || `image/${ext === 'jpg' ? 'jpeg' : ext}`;
        pendingAttachments.push({
          name: file.name,
          type: "image",
          data: b64,
          mime,
          preview: `data:${mime};base64,${b64}`,
        });
        render();
      } else {
        // Document: extract text via backend
        try {
          const result = await invoke<string>("upload_file", { name: file.name, dataBase64: b64 });
          if (result.startsWith("IMAGE:")) {
            const parts = result.split(":");
            pendingAttachments.push({ name: file.name, type: "image", data: parts[2], mime: parts[1] });
          } else {
            pendingAttachments.push({ name: file.name, type: "document", data: result });
          }
          render();
        } catch (e) {
          pendingAttachments.push({ name: file.name, type: "document", data: `[Error reading ${file.name}: ${e}]` });
          render();
        }
      }
    };
    reader.readAsArrayBuffer(file);
  }
}

// Voice-only conversation (no text UI, speaks response)
async function voiceChat(text: string) {
  try {
    await invoke("send_message", { message: text });
    voiceInitiatedMessage = true;
  } catch (e) {
    voiceTranscript = `Error: ${e}`;
    voiceState = "idle";
    render();
  }
}

// ── Conversations ──
async function loadConversations() {
  try { conversations = await invoke<Conversation[]>("list_conversations"); } catch { conversations = []; }
}

async function saveCurrentConvo() {
  if (messages.length < 2) return;
  if (!currentConvoId) currentConvoId = uid();
  const title = messages[0]?.content.slice(0, 40) || "New Chat";
  const convo: Conversation = { id: currentConvoId, title, messages: [...messages], created: conversations.find(c => c.id === currentConvoId)?.created || new Date().toISOString(), updated: new Date().toISOString() };
  try { await invoke("save_conversation", { id: currentConvoId, data: convo }); } catch {}
  await loadConversations();
}

function loadConvo(id: string) {
  const convo = conversations.find(c => c.id === id);
  if (!convo) return;
  messages = [...convo.messages]; currentConvoId = id; currentPanel = "chat"; render();
}

function newChat() { messages = []; currentConvoId = ""; currentPanel = "chat"; invoke("clear_history"); render(); }

async function deleteConvo(id: string) {
  try { await invoke("delete_conversation", { id }); } catch {}
  if (currentConvoId === id) newChat();
  await loadConversations(); render();
}

// ── Rendering Helpers ──
function stripToolTokens(text: string): string {
  return text.replace(/\[TOOL:\w+[^\]]*\]/g, "").replace(/\[CREATE_TOOL:\w+\][\s\S]*?\[\/CREATE_TOOL\]/g, "").replace(/\n{3,}/g, "\n\n").trim();
}

function md(text: string): string {
  let s = text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  s = s.replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) => {
    let h: string; try { h = lang ? hljs.highlight(code.trim(), { language: lang }).value : hljs.highlightAuto(code.trim()).value; } catch { h = code.trim(); }
    return `<pre><code class="hljs">${lang ? `<span class="code-lang">${lang}</span>` : ""}${h}</code></pre>`;
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

// ── Streaming ──
function updateStreamingBubble() {
  const chatArea = document.getElementById("chat-area");
  if (!chatArea) return;
  let bubble = document.getElementById("streaming-bubble");
  if (!bubble) { bubble = document.createElement("div"); bubble.id = "streaming-bubble"; bubble.className = "message assistant anim-in"; bubble.innerHTML = '<div class="msg-label">Nebula</div><div class="tool-area"></div><div class="msg-body"></div>'; chatArea.appendChild(bubble); }
  const body = bubble.querySelector(".msg-body");
  if (body) body.innerHTML = md(stripToolTokens(streamBuffer));
  chatArea.scrollTop = chatArea.scrollHeight;
}

function updateToolDisplay() {
  let bubble = document.getElementById("streaming-bubble");
  if (!bubble) { const chatArea = document.getElementById("chat-area"); if (!chatArea) return; bubble = document.createElement("div"); bubble.id = "streaming-bubble"; bubble.className = "message assistant anim-in"; bubble.innerHTML = '<div class="msg-label">JARVIS</div><div class="tool-area"></div><div class="msg-body"></div>'; chatArea.appendChild(bubble); }
  const toolArea = bubble.querySelector(".tool-area");
  if (toolArea) {
    toolArea.innerHTML = toolHistory.map(t => {
      const icon = t.status === "running" ? '<span class="tool-spin">⚙️</span>' : '<span class="tool-done">✓</span>';
      const res = t.result ? `<div class="tool-res">${t.result.replace(/</g, "&lt;").slice(0, 120)}</div>` : '';
      return `<div class="tool-card">${icon} <span class="tool-name">${t.name}</span> <span class="tool-args">${t.args.replace(/</g, "&lt;")}</span>${res}</div>`;
    }).join("");
  }
  document.getElementById("chat-area")?.scrollTo(0, 99999);
}

// ── System State HUD ──
let systemState: any = null;
async function pollSystemState() {
  try { systemState = await invoke("get_system_state"); } catch {}
}

function renderHUD(): string {
  if (!systemState) return '';
  const hw = systemState.hardware || {};
  const ws = systemState.workspace || {};
  const periph = systemState.peripherals || {};
  const procs = systemState.processes || {};
  return `<div class="hud-bar">
    <span class="hud-item">CPU <span class="hud-val">${hw.cpu_usage?.toFixed(0) || 0}%</span></span>
    <span class="hud-item">RAM <span class="hud-val">${hw.memory_percent?.toFixed(0) || 0}%</span></span>
    <span class="hud-item">DISK <span class="hud-val">${hw.disk_free_gb?.toFixed(0) || '?'}G</span></span>
    <span class="hud-item">MODE <span class="hud-val">${(ws.current_mode || 'general').toUpperCase()}</span></span>
    <span class="hud-item"><span class="hud-dot ${periph.microphone_active ? 'on' : ''}"></span>MIC</span>
    <span class="hud-item"><span class="hud-dot ${periph.webcam_active ? 'on' : ''}"></span>CAM</span>
  </div>`;
}

// ── Main Render ──
function render() {
  const app = document.getElementById("app")!;
  app.innerHTML = `
    <div class="layout">
      <aside class="sidebar ${sidebarOpen ? "open" : "closed"}">
        <div class="sidebar-header">
          <button class="sidebar-new" id="btn-new">+ New Chat</button>
        </div>
        <div class="sidebar-list" id="sidebar-list">
          ${conversations.map(c => `
            <div class="sidebar-item ${c.id === currentConvoId ? "active" : ""}" data-convo="${c.id}">
              <span class="sidebar-title">${c.title}</span>
              <button class="sidebar-del" data-del="${c.id}">×</button>
            </div>
          `).join("") || '<div class="sidebar-empty">No conversations yet</div>'}
        </div>
      </aside>
      <div class="main">
        ${renderHUD()}
        <header class="topbar">
          <div class="topbar-left">
            <button class="btn-toggle-sidebar" id="btn-sidebar">${sidebarOpen ? "◀" : "☰"}</button>
            <div class="topbar-title">Nebula</div>
            <nav class="topbar-nav">
              <button class="nav-tab ${currentPanel === "chat" ? "active" : ""}" data-panel="chat">Chat</button>
              <button class="nav-tab ${currentPanel === "voice" ? "active" : ""}" data-panel="voice">Voice</button>
              <button class="nav-tab ${currentPanel === "vault" ? "active" : ""}" data-panel="vault">Vault</button>
              <button class="nav-tab ${currentPanel === "settings" ? "active" : ""}" data-panel="settings">Settings</button>
            </nav>
          </div>
          <div class="topbar-actions">
            <button class="btn-icon" id="btn-theme">${theme === "dark" ? "☀️" : "🌙"}</button>
          </div>
        </header>
        <main class="content">
          ${currentPanel === "chat" ? renderChat() : ""}
          ${currentPanel === "voice" ? renderVoiceMode() : ""}
          ${currentPanel === "vault" ? renderVault() : ""}
          ${currentPanel === "settings" ? renderSettings() : ""}
        </main>
        ${currentPanel === "chat" ? renderInput() : ""}
      </div>
    </div>`;
  document.getElementById("chat-area")?.scrollTo(0, 99999);
  attachListeners();
}

function renderChat(): string {
  const msgs = messages.map(m => {
    const cls = m.role === "user" ? "user" : "assistant";
    const label = m.role === "user" ? "You" : "Nebula";
    const body = m.role === "user" ? m.content.replace(/</g, "&lt;").replace(/\n/g, "<br>") : md(stripToolTokens(m.content));
    const tools = m.tools?.length ? `<div class="tool-area">${m.tools.map(t => `<div class="tool-card"><span class="tool-done">✓</span> <span class="tool-name">${t.name}</span> <span class="tool-args">${t.args.replace(/</g, "&lt;")}</span>${t.result ? `<div class="tool-res">${t.result.replace(/</g, "&lt;").slice(0, 120)}</div>` : ""}</div>`).join("")}</div>` : "";
    return `<div class="message ${cls} anim-in"><div class="msg-label">${label}</div>${tools}<div class="msg-body">${body}</div></div>`;
  }).join("");
  const thinking = isStreaming && !streamBuffer ? '<div class="message assistant anim-in"><div class="msg-label">Nebula</div><div class="msg-body"><span class="dots"></span></div></div>' : "";
  return `<div class="chat-area" id="chat-area">${messages.length === 0 ? `<div class="welcome">
    <div class="welcome-title">Nebula</div>
    <div class="welcome-sub">Neural Engine for Bookkeeping, Understanding, Language & Automation</div>
    <div class="welcome-shortcuts">
      <div class="welcome-shortcut" data-shortcut="What's on my screen?">
        <div class="welcome-shortcut-icon">🖥</div>
        <div class="welcome-shortcut-label">Screen</div>
      </div>
      <div class="welcome-shortcut" data-shortcut="What's the weather like?">
        <div class="welcome-shortcut-icon">☀️</div>
        <div class="welcome-shortcut-label">Weather</div>
      </div>
      <div class="welcome-shortcut" data-shortcut="Search my vault for recent notes">
        <div class="welcome-shortcut-icon">📝</div>
        <div class="welcome-shortcut-label">Vault</div>
      </div>
      <div class="welcome-shortcut" data-shortcut="Give me a system status report">
        <div class="welcome-shortcut-icon">💻</div>
        <div class="welcome-shortcut-label">System</div>
      </div>
    </div>
  </div>` : ""}${msgs}${thinking}</div>`;
}

function renderInput(): string {
  const attachmentPills = pendingAttachments.map((att, i) => {
    if (att.type === "image" && att.preview) {
      return `<div class="attach-pill"><img src="${att.preview}" class="attach-thumb"><span>${att.name}</span><button class="attach-remove" data-att-idx="${i}">×</button></div>`;
    }
    return `<div class="attach-pill"><span class="attach-icon">📄</span><span>${att.name}</span><button class="attach-remove" data-att-idx="${i}">×</button></div>`;
  }).join("");

  return `<footer class="input-footer">
    <div class="quick-actions">
      <button class="qa-btn" data-qa="clipboard" title="Paste clipboard">📋 Clipboard</button>
      <button class="qa-btn" data-qa="sysinfo" title="System info">💻 System</button>
      <button class="qa-btn" data-qa="screen" title="Analyze screen">🖥 Screen</button>
      <button class="qa-btn" data-qa="weather" title="Weather">☀️ Weather</button>
      <button class="qa-btn" data-qa="note" title="Quick note">📝 Note</button>
      <button class="qa-btn" data-qa="search" title="Search vault">🔍 Vault</button>
    </div>
    ${attachmentPills ? `<div class="attach-bar">${attachmentPills}</div>` : ""}
    <div class="input-bar">
      <button class="btn-attach" id="btn-attach" title="Attach file">📎</button>
      <input type="file" id="file-input" multiple accept=".pdf,.docx,.xlsx,.pptx,.doc,.odt,.xls,.ods,.ppt,.odp,.rtf,.csv,.txt,.md,.png,.jpg,.jpeg,.gif,.webp" style="display:none">
      <textarea id="chat-input" placeholder="Ask Nebula anything..." ${isStreaming ? "disabled" : ""}></textarea>
      <button class="btn-send" id="btn-send" ${isStreaming ? "disabled" : ""}>↑</button>
    </div>
    <div class="input-hint">Enter to send · Shift+Enter for newline · 📎 to attach files</div>
  </footer>`;
}

function renderVoiceMode(): string {
  const stateText = {
    idle: "Ready — start speaking",
    wake: "Listening...",
    listening: "Recording...",
    processing: "Processing...",
  }[voiceState] || "Idle";

  const pulseClass = voiceState === "wake" || voiceState === "listening" ? "pulse-active" : "";
  const orbClass = voiceModeActive ? "orb-active" : "";

  return `<div class="voice-panel">
    <div class="voice-orb-container">
      <div class="voice-orb ${orbClass} ${pulseClass}">
        <div class="voice-orb-inner"></div>
        <div class="voice-orb-ring"></div>
        <div class="voice-orb-ring ring-2"></div>
      </div>
    </div>
    <div class="voice-status">${stateText}</div>
    ${voiceTranscript ? `<div class="voice-transcript">"${voiceTranscript}"</div>` : ""}
    <div class="voice-controls">
      ${!voiceModeActive
        ? `<button class="voice-btn start" id="btn-voice-start">Start Voice Mode</button>`
        : `<button class="voice-btn stop" id="btn-voice-stop">Stop</button>`
      }
    </div>
    <div class="voice-info">
      <p>Wake words: "Hey Nebula", "Nebula", "Yo Nebula"</p>
      <p>Uses Whisper for speech recognition • Piper for TTS response</p>
    </div>
  </div>`;
}

function renderVault(): string {
  const results = vaultResults.map(r => `<div class="vault-result"><div class="vault-header"><span class="vault-path">${r.path}</span><span class="vault-score">${(r.score*100).toFixed(0)}%</span></div><div class="vault-section">${r.header}</div><div class="vault-snippet">${r.text.replace(/</g,"&lt;").slice(0,200)}</div><button class="vault-ask" data-vault-text="${encodeURIComponent(r.text.slice(0,500))}">Ask about this</button></div>`).join("");
  return `<div class="panel-content"><div class="vault-bar"><input type="text" id="vault-input" class="panel-input" placeholder="Search vault..." value="${vaultQuery}"><button class="btn-send" id="btn-vault-search">Search</button><button class="btn-icon" id="btn-reindex">🔄</button></div><div class="vault-results">${!vaultQuery ? '<div class="panel-empty">Search your Obsidian vault.</div>' : results || '<div class="panel-empty">No results.</div>'}</div></div>`;
}

function renderSettings(): string {
  return `<div class="panel-content settings"><h3>Settings</h3>
    <div class="form-group"><label>API Key</label><input type="password" id="set-api-key" class="panel-input"></div>
    <div class="form-group"><label>Base URL</label><input type="text" id="set-base-url" class="panel-input"></div>
    <div class="form-group"><label>Model</label><input type="text" id="set-model" class="panel-input"></div>
    <div class="form-group"><label>Fast Model</label><input type="text" id="set-fast-model" class="panel-input"></div>
    <div class="form-group"><label>Max Tokens</label><input type="number" id="set-max-tokens" class="panel-input"></div>
    <div class="form-group"><label>Vault Path</label><input type="text" id="set-vault-path" class="panel-input"></div>
    <h4>Google</h4>
    <div class="form-group"><label>Search API Key</label><input type="text" id="set-google-search-key" class="panel-input" placeholder="From Google Cloud Console"></div>
    <div class="form-group"><label>Search CX</label><input type="text" id="set-google-cx" class="panel-input" placeholder="Custom Search Engine ID"></div>
    <div class="form-group"><label>OAuth Client ID</label><input type="text" id="set-google-client-id" class="panel-input" placeholder="For Calendar"></div>
    <div class="form-group"><label>OAuth Client Secret</label><input type="password" id="set-google-secret" class="panel-input"></div>
    <h4>Voice</h4>
    <div class="form-group"><label>Piper Model Path</label><input type="text" id="set-voice-model" class="panel-input"></div>
    <div class="form-group"><label>Speed</label><input type="number" id="set-voice-speed" class="panel-input" step="0.1"></div>
    <div class="form-actions"><button class="btn-send" id="btn-save-settings">Save</button><span id="settings-status"></span></div>
  </div>`;
}

// ── Listeners ──
function attachListeners() {
  // Nav
  document.querySelectorAll("[data-panel]").forEach(el => el.addEventListener("click", () => { currentPanel = (el as HTMLElement).dataset.panel as typeof currentPanel; render(); if (currentPanel === "settings") loadSettings(); }));
  document.getElementById("btn-sidebar")?.addEventListener("click", () => { sidebarOpen = !sidebarOpen; localStorage.setItem("jarvis-sidebar", sidebarOpen ? "open" : "closed"); render(); });
  document.getElementById("btn-new")?.addEventListener("click", newChat);

  // Conversations
  document.querySelectorAll("[data-convo]").forEach(el => el.addEventListener("click", (e) => { if (!(e.target as HTMLElement).classList.contains("sidebar-del")) loadConvo((el as HTMLElement).dataset.convo!); }));
  document.querySelectorAll("[data-del]").forEach(el => el.addEventListener("click", (e) => { e.stopPropagation(); deleteConvo((el as HTMLElement).dataset.del!); }));

  // Chat
  const input = document.getElementById("chat-input") as HTMLTextAreaElement;
  document.getElementById("btn-send")?.addEventListener("click", () => { if (input?.value.trim()) { sendMessage(input.value.trim()); input.value = ""; input.style.height = "auto"; } });
  input?.addEventListener("keydown", (e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); if (input.value.trim()) { sendMessage(input.value.trim()); input.value = ""; input.style.height = "auto"; } } });
  input?.addEventListener("input", () => { input.style.height = "auto"; input.style.height = Math.min(input.scrollHeight, 160) + "px"; });

  // File upload
  const fileInput = document.getElementById("file-input") as HTMLInputElement;
  document.getElementById("btn-attach")?.addEventListener("click", () => fileInput?.click());
  fileInput?.addEventListener("change", () => { if (fileInput.files?.length) { handleFileUpload(fileInput.files); fileInput.value = ""; } });
  document.querySelectorAll("[data-att-idx]").forEach(el => el.addEventListener("click", () => {
    const idx = parseInt((el as HTMLElement).dataset.attIdx || "0");
    pendingAttachments.splice(idx, 1);
    render();
  }));

  // Quick actions
  document.querySelectorAll("[data-qa]").forEach(el => el.addEventListener("click", () => {
    const qa = (el as HTMLElement).dataset.qa!;
    if (qa === "clipboard") sendMessage("What's on my clipboard? [TOOL:clipboard action=get]");
    else if (qa === "sysinfo") sendMessage("Give me a system status report");
    else if (qa === "screen") sendMessage("Take a screenshot and tell me what's on my screen [TOOL:analyze_screen]");
    else if (qa === "weather") sendMessage("What's the weather like? [TOOL:weather]");
    else if (qa === "note") { const note = prompt("Quick note:"); if (note) sendMessage(`Save this note: ${note}`); }
    else if (qa === "search") { currentPanel = "vault"; render(); }
  }));

  // Theme
  document.getElementById("btn-theme")?.addEventListener("click", () => { theme = theme === "dark" ? "light" : "dark"; localStorage.setItem("jarvis-theme", theme); applyTheme(); render(); });

  // Vault
  const vi = document.getElementById("vault-input") as HTMLInputElement;
  document.getElementById("btn-vault-search")?.addEventListener("click", () => { if (vi?.value.trim()) searchVault(vi.value.trim()); });
  vi?.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); if (vi.value.trim()) searchVault(vi.value.trim()); } });
  document.getElementById("btn-reindex")?.addEventListener("click", async () => { const b = document.getElementById("btn-reindex"); if (b) b.textContent = "⏳"; try { await invoke("reindex_vault", { force: true }); if (b) b.textContent = "✓"; setTimeout(() => { if (b) b.textContent = "🔄"; }, 2000); } catch { if (b) b.textContent = "✗"; } });
  document.querySelectorAll("[data-vault-text]").forEach(el => el.addEventListener("click", () => { const t = decodeURIComponent((el as HTMLElement).dataset.vaultText!); currentPanel = "chat"; render(); sendMessage(`Based on this vault note:\n\n${t}\n\nSummarize.`); }));

  // Settings
  document.getElementById("btn-save-settings")?.addEventListener("click", saveSettings);

  // Focus
  if (currentPanel === "chat" && !isStreaming) input?.focus();
  if (currentPanel === "vault") vi?.focus();

  // Voice mode
  document.getElementById("btn-voice-start")?.addEventListener("click", async () => {
    try { await invoke("start_voice_mode"); voiceModeActive = true; voiceState = "idle"; render(); }
    catch (e) { voiceTranscript = `Error: ${e}`; render(); }
  });
  document.getElementById("btn-voice-stop")?.addEventListener("click", async () => {
    try { await invoke("stop_voice_mode"); voiceModeActive = false; voiceState = "idle"; render(); }
    catch (e) { voiceTranscript = `Error: ${e}`; render(); }
  });

  // Shortcuts
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey && e.key === "l") { e.preventDefault(); newChat(); }
    if (e.ctrlKey && e.key === "k") { e.preventDefault(); currentPanel = "vault"; render(); }
    if (e.ctrlKey && e.key === "n") { e.preventDefault(); newChat(); }
  });

  // Welcome shortcuts
  document.querySelectorAll("[data-shortcut]").forEach(el => el.addEventListener("click", () => {
    sendMessage((el as HTMLElement).dataset.shortcut!);
  }));
}

// ── Settings load/save ──
async function loadSettings() {
  try {
    const c = await invoke<AppConfig>("get_config");
    const s = (id: string, v: string) => { const el = document.getElementById(id) as HTMLInputElement; if (el) el.value = v; };
    s("set-api-key", c.api_key); s("set-base-url", c.base_url); s("set-model", c.model);
    s("set-fast-model", c.fast_model); s("set-max-tokens", String(c.max_tokens));
    s("set-vault-path", c.vault_path); s("set-voice-model", c.voice.model);
    s("set-voice-speed", String(c.voice.speed));
    s("set-google-search-key", c.google?.search_api_key || "");
    s("set-google-cx", c.google?.search_cx || "");
    s("set-google-client-id", c.google?.client_id || "");
    s("set-google-secret", c.google?.client_secret || "");
  } catch {}
}

async function saveSettings() {
  const g = (id: string) => (document.getElementById(id) as HTMLInputElement)?.value || "";
  const config: AppConfig = {
    api_key: g("set-api-key"), base_url: g("set-base-url"), model: g("set-model"),
    fast_model: g("set-fast-model"), fallback_models: ["anthropic.claude-sonnet-4-6", "us.anthropic.claude-sonnet-4-5-20250929-v1:0"],
    max_tokens: parseInt(g("set-max-tokens")) || 4096, fast_max_tokens: 2048,
    vault_path: g("set-vault-path"),
    voice: { enabled: true, model: g("set-voice-model"), speed: parseFloat(g("set-voice-speed")) || 1.1 },
    theme,
    google: { search_api_key: g("set-google-search-key"), search_cx: g("set-google-cx"), client_id: g("set-google-client-id"), client_secret: g("set-google-secret") },
  };
  try { await invoke("save_config", { config }); const st = document.getElementById("settings-status"); if (st) { st.textContent = "Saved ✓"; setTimeout(() => { st.textContent = ""; }, 2000); } }
  catch (e) { const st = document.getElementById("settings-status"); if (st) st.textContent = `Error: ${e}`; }
}

function applyTheme() { document.documentElement.setAttribute("data-theme", theme); }

// ── Init ──
applyTheme();
render();
(async () => {
  try { await loadConversations(); } catch {}
  try { await pollSystemState(); } catch {}
  render();
  setInterval(pollSystemState, 5000);
})();
