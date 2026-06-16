use chrono::Local;
use futures::StreamExt;
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::tools::{parse_tool_calls, ToolRegistry};
use crate::vault::VaultIndex;

const SYSTEM_PROMPT: &str = "You are Nebula, an autonomous AI assistant with full desktop control. Act immediately, never ask. Be concise.\n\nTOOLS: [TOOL:name arg=value]\nCore: run_command, read_file, write_file, list_files, find_files, replace_in_file, git, multi_command\nWeb: google_search, web_fetch, open_url\nSystem: clipboard, notify, datetime, calc, system_info, processes, kill_process, network, bluetooth\nDesktop: screenshot, analyze_screen, volume, brightness, window, media, timer, weather, app_launch, quick_action\nMemory: remember, knowledge_search, knowledge_save, recall, save_decision, save_idea, save_person, todo\nApps: obsidian (open/create/search/daily), spotify, file_manager, calendar";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub images: Vec<ImageData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    pub mime_type: String,
    pub data: String, // base64
}

pub struct Brain {
    client: Client,
    history: Vec<Message>,
    config: Config,
    tools: ToolRegistry,
    vault: Option<std::sync::Arc<VaultIndex>>,
    cached_sys_state: String,
    cached_sys_time: std::time::Instant,
}

impl Brain {
    pub fn new(config: Config) -> Self {
        Self {
            client: Client::new(),
            history: Vec::new(),
            tools: ToolRegistry::new(),
            vault: None,
            config,
            cached_sys_state: String::new(),
            cached_sys_time: std::time::Instant::now(),
        }
    }

    pub fn set_vault(&mut self, vault: std::sync::Arc<VaultIndex>) {
        self.vault = Some(vault);
    }

    pub async fn chat_stream(
        &mut self,
        text: &str,
        tx: mpsc::UnboundedSender<StreamEvent>,
    ) -> String {
        self.chat_stream_with_images(text, vec![], tx).await
    }

    pub async fn chat_stream_with_images(
        &mut self,
        text: &str,
        images: Vec<ImageData>,
        tx: mpsc::UnboundedSender<StreamEvent>,
    ) -> String {
        self.history.push(Message {
            role: "user".into(),
            content: text.into(),
            images: images.clone(),
        });

        let system = self.build_system();
        let messages = self.build_messages();
        let model = self.config.model.clone();

        let response = self
            .stream_api(&system, &messages, &model, self.config.max_tokens, &tx)
            .await;

        self.history.push(Message {
            role: "assistant".into(),
            content: response.clone(),
            images: vec![],
        });

        // Log interaction to daily file
        self.log_interaction(text, &response);

        // Handle tool calls
        self.handle_tools(&response, &tx, 0).await;

        let _ = tx.send(StreamEvent::Done(response.clone()));
        response
    }

    async fn handle_tools(
        &mut self,
        response: &str,
        tx: &mpsc::UnboundedSender<StreamEvent>,
        depth: usize,
    ) {
        if depth > 7 {
            return;
        }

        // Check for tool creation
        let create_re = Regex::new(r"(?s)\[CREATE_TOOL:(\w+)\](.*?)\[/CREATE_TOOL\]").unwrap();
        let mut created = Vec::new();
        for cap in create_re.captures_iter(response) {
            let name = cap[1].trim().to_string();
            let code = cap[2].trim().to_string();
            if !name.is_empty() && !code.is_empty() {
                let result = self.tools.create_tool(&name, &code);
                let _ = tx.send(StreamEvent::ToolResult {
                    name: format!("create:{}", name),
                    result: result.clone(),
                });
                created.push(name);
            }
        }

        // Check for tool calls
        let calls = parse_tool_calls(response);
        if calls.is_empty() && created.is_empty() {
            return;
        }

        let mut all_results = Vec::new();

        for (name, args) in &calls {
            let args_display: String = args
                .iter()
                .map(|(k, v)| format!("{}={}", k, &v[..v.len().min(40)]))
                .collect::<Vec<_>>()
                .join(", ");

            let _ = tx.send(StreamEvent::ToolStart {
                name: name.clone(),
                args: args_display,
            });

            let result = self.tools.execute(name, args);
            let short = if result.len() > 300 {
                format!("{}...", &result[..300])
            } else {
                result.clone()
            };

            let _ = tx.send(StreamEvent::ToolResult {
                name: name.clone(),
                result: short,
            });

            // Higher limits for content-heavy tools
            let max_len = match name.as_str() {
                "screenshot" | "analyze_screen" | "web_fetch" | "google_search"
                | "read_file" | "knowledge_search" | "recall" | "run_command"
                | "find_files" | "obsidian" => 2000,
                _ => 800,
            };
            all_results.push(format!("[{}]: {}", name, &result[..result.len().min(max_len)]));
        }

        if all_results.is_empty() && created.is_empty() {
            return;
        }

        // Build feedback
        let feedback = if !all_results.is_empty() {
            all_results.join("\n")
        } else {
            format!("Tool '{}' created. Use it now if needed.", created[0])
        };

        self.history.push(Message {
            role: "user".into(),
            content: feedback,
            images: vec![],
        });

        // Use fast model for tool followups
        let system = self.build_system();
        let messages = self.build_messages();
        let fast_model = self.config.fast_model.clone();

        let _ = tx.send(StreamEvent::Token("\n".into()));

        let followup = self
            .stream_api(&system, &messages, &fast_model, self.config.fast_max_tokens, &tx)
            .await;

        self.history.push(Message {
            role: "assistant".into(),
            content: followup.clone(),
            images: vec![],
        });

        // Recurse for chained calls
        Box::pin(self.handle_tools(&followup, tx, depth + 1)).await;
    }

    async fn stream_api(
        &self,
        system: &Value,
        messages: &[Value],
        model: &str,
        max_tokens: u32,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> String {
        let models_to_try: Vec<String> = std::iter::once(model.to_string())
            .chain(self.config.fallback_models.iter().cloned())
            .collect();

        // Per-model retry budget for transient errors (429/5xx/timeout) before
        // moving on. Free-tier limits are usually per-minute, so a couple of
        // short backoff retries on the SAME model often clears a 429 without
        // burning the whole fallback chain.
        const MAX_RETRIES_PER_MODEL: u32 = 2;

        for m in &models_to_try {
            let mut attempt: u32 = 0;
            loop {
                match self.try_stream(system, messages, m, max_tokens, tx).await {
                    Ok(response) => return response,
                    Err(ApiError::Fatal(e)) => {
                        // Bad key / malformed request / auth — sweeping other
                        // models won't help. Abort the whole chain.
                        eprintln!("[brain] Model {} FATAL: {} (aborting fallback)", m, e);
                        let msg = format!("[Request rejected: {}]", e);
                        let _ = tx.send(StreamEvent::Token(msg.clone()));
                        return msg;
                    }
                    Err(ApiError::Streamed(partial)) => {
                        // We already streamed tokens to the user; retrying would
                        // double-print. Keep what we have.
                        eprintln!("[brain] Model {} stream truncated, keeping {} chars", m, partial.len());
                        return partial;
                    }
                    Err(ApiError::Retryable { reason, retry_after }) => {
                        eprintln!("[brain] Model {} retryable ({}), attempt {}/{}", m, reason, attempt + 1, MAX_RETRIES_PER_MODEL + 1);
                        if attempt >= MAX_RETRIES_PER_MODEL {
                            break; // give up on this model, try the next
                        }
                        let delay = retry_after.unwrap_or_else(|| Self::backoff_delay(attempt));
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                }
            }
            // brief pause before switching models
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }

        let msg = "[All models rate-limited or unavailable — try again in a moment]".to_string();
        let _ = tx.send(StreamEvent::Token(msg.clone()));
        msg
    }

    /// Exponential backoff with deterministic jitter (no rng dep).
    /// attempt 0 -> ~0.6s, 1 -> ~1.4s, 2 -> ~3.0s
    fn backoff_delay(attempt: u32) -> std::time::Duration {
        let base_ms = 500u64 * (1u64 << attempt); // 500, 1000, 2000...
        // jitter derived from the attempt so retries don't thundering-herd
        let jitter = 100 + (attempt as u64 * 137) % 400;
        std::time::Duration::from_millis(base_ms + jitter)
    }

    async fn try_stream(
        &self,
        system: &Value,
        messages: &[Value],
        model: &str,
        max_tokens: u32,
        tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<String, ApiError> {
        let body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": messages,
            "stream": true,
        });

        let url = format!("{}/v1/messages", self.config.base_url);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                // Network/timeout — worth retrying or falling back.
                ApiError::Retryable { reason: format!("request failed: {}", e), retry_after: None }
            })?;

        let status = response.status();
        if status.as_u16() == 429 {
            // Honor Retry-After (seconds) if the server sent one.
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(std::time::Duration::from_secs);
            return Err(ApiError::Retryable { reason: "rate limited (429)".into(), retry_after });
        }
        if status.is_server_error() {
            // 5xx — provider-side, retry / fall back.
            let body = response.text().await.unwrap_or_default();
            return Err(ApiError::Retryable {
                reason: format!("HTTP {}: {}", status, &body[..200.min(body.len())]),
                retry_after: None,
            });
        }
        if !status.is_success() {
            // 4xx other than 429 (400 bad request, 401/403 auth) — fatal.
            let body = response.text().await.unwrap_or_default();
            return Err(ApiError::Fatal(format!("HTTP {}: {}", status, &body[..200.min(body.len())])));
        }

        // Parse SSE stream
        let mut full_response = String::new();
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    // Mid-stream failure. If we've already emitted tokens to the
                    // user, retrying would double-print — keep the partial.
                    if !full_response.is_empty() {
                        return Err(ApiError::Streamed(full_response));
                    }
                    return Err(ApiError::Retryable {
                        reason: format!("stream error: {}", e),
                        retry_after: None,
                    });
                }
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete SSE lines
            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim_end().to_string();
                buffer = buffer[line_end + 1..].to_string();

                if line.starts_with("data: ") {
                    let data = &line[6..];
                    if data == "[DONE]" {
                        break;
                    }

                    if let Ok(event) = serde_json::from_str::<Value>(data) {
                        // Extract text from content_block_delta events
                        if event.get("type").and_then(|t| t.as_str())
                            == Some("content_block_delta")
                        {
                            if let Some(text) = event
                                .get("delta")
                                .and_then(|d| d.get("text"))
                                .and_then(|t| t.as_str())
                            {
                                full_response.push_str(text);
                                let _ = tx.send(StreamEvent::Token(text.to_string()));
                            }
                        }
                    }
                }
            }
        }

        eprintln!("[brain] Stream complete, {} chars", full_response.len());

        if full_response.is_empty() {
            return Err(ApiError::Retryable { reason: "empty response".into(), retry_after: None });
        }

        Ok(full_response)
    }

    fn build_system(&mut self) -> Value {
        let date = Local::now().format("%Y-%m-%d %H:%M").to_string();

        // Get memory context from vault
        let memory_ctx = self
            .vault
            .as_ref()
            .map(|v| v.get_context())
            .unwrap_or_default();

        // Get system state snapshot (quick — non-blocking summary)
        let sys_state = self.get_quick_system_state();

        // Load learnings from retrospectives
        let learnings = self.load_learnings();

        let mut dynamic_parts = Vec::new();
        if !sys_state.is_empty() {
            dynamic_parts.push(format!("Environment:\n{}", sys_state));
        }
        if !memory_ctx.is_empty() {
            dynamic_parts.push(format!("Memory:\n{}", memory_ctx));
        }
        if !learnings.is_empty() {
            dynamic_parts.push(format!("Learnings from past sessions:\n{}", learnings));
        }
        dynamic_parts.push(format!("Date: {}", date));

        // Static part (cacheable) + dynamic part
        json!([
            {
                "type": "text",
                "text": SYSTEM_PROMPT,
                "cache_control": {"type": "ephemeral"}
            },
            {
                "type": "text",
                "text": dynamic_parts.join("\n\n")
            }
        ])
    }

    fn get_quick_system_state(&mut self) -> String {
        // Cache for 5 seconds — avoid shelling out on every message
        if self.cached_sys_time.elapsed().as_secs() < 5 && !self.cached_sys_state.is_empty() {
            return self.cached_sys_state.clone();
        }

        let mut parts = Vec::new();

        // Active window
        if let Ok(output) = std::process::Command::new("sh").arg("-c")
            .arg("xdotool getactivewindow getwindowname 2>/dev/null")
            .output() {
            let win = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !win.is_empty() && win != "unknown" {
                parts.push(format!("Active: {}", &win[..win.len().min(40)]));
            }
        }

        // Quick memory check
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                if line.starts_with("MemAvailable:") {
                    let kb: u64 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                    parts.push(format!("RAM free: {}GB", kb / 1_048_576));
                    break;
                }
            }
        }

        self.cached_sys_state = parts.join(" | ");
        self.cached_sys_time = std::time::Instant::now();
        self.cached_sys_state.clone()
    }

    fn load_learnings(&self) -> String {
        let vault_path = self.config.vault_path_expanded();
        let learnings_file = vault_path.join("memory/learnings.md");

        match std::fs::read_to_string(&learnings_file) {
            Ok(content) => {
                // Extract just the bullet points, skip frontmatter
                let lines: Vec<&str> = content.lines()
                    .filter(|l| l.starts_with("- "))
                    .collect();
                // Take last 20 learnings (most recent)
                let recent: Vec<&str> = lines.iter().rev().take(20).rev().cloned().collect();
                recent.join("\n")
            }
            Err(_) => String::new(),
        }
    }

    fn build_messages(&self) -> Vec<Value> {
        let window: Vec<&Message> = self.history.iter().rev().take(20).collect::<Vec<_>>();
        let window: Vec<&Message> = window.into_iter().rev().collect();

        let mut messages: Vec<Value> = Vec::new();

        for msg in window {
            let role = &msg.role;
            let content = &msg.content;

            // Build content blocks
            let mut content_blocks: Vec<Value> = Vec::new();

            // Add images first (for vision)
            for img in &msg.images {
                content_blocks.push(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": &img.mime_type,
                        "data": &img.data
                    }
                }));
            }

            // Add text
            content_blocks.push(json!({"type": "text", "text": content}));

            if let Some(last) = messages.last_mut() {
                if last.get("role").and_then(|r| r.as_str()) == Some(role) && msg.images.is_empty() {
                    // Merge consecutive same-role text-only messages
                    if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                        arr.push(json!({"type": "text", "text": content}));
                    }
                    continue;
                }
            }

            messages.push(json!({
                "role": role,
                "content": content_blocks
            }));
        }

        // Ensure first message is from user
        if messages.first().and_then(|m| m.get("role")).and_then(|r| r.as_str()) != Some("user") {
            if !messages.is_empty() {
                messages.remove(0);
            }
        }

        messages
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }

    /// Proactive reasoning: given an observed environment event, decide whether
    /// Nebula should surface a short, dismissible nudge — and what it should say.
    ///
    /// This is the autonomous core: it uses the fast model, the vault memory,
    /// and the user's learnings to judge relevance. Returns `None` when nothing
    /// is worth interrupting for (the common case), so the caller stays quiet.
    pub async fn consider_proactive(&self, event_desc: &str) -> Option<String> {
        let memory_ctx = self
            .vault
            .as_ref()
            .map(|v| v.get_context())
            .unwrap_or_default();
        let learnings = self.load_learnings();

        let prompt = format!(
            "You passively observe the user's desktop. An event just occurred:\n{event}\n\n\
            What you know about the user:\nMemory: {memory}\nLearnings: {learnings}\n\n\
            Decide if a SHORT proactive suggestion is genuinely helpful right now. \
            Most of the time it is NOT — silence is the default. Only speak up if you \
            can offer concrete, timely value the user would thank you for.\n\n\
            Reply with EXACTLY one line:\n\
            SKIP   (if nothing useful — this is usually correct)\n\
            or\n\
            SAY: <one short sentence, under 120 chars, no preamble>",
            event = event_desc,
            memory = &memory_ctx[..memory_ctx.len().min(800)],
            learnings = &learnings[..learnings.len().min(600)],
        );

        let body = json!({
            "model": &self.config.fast_model,
            "max_tokens": 80,
            "system": "You are Nebula's proactive judgment. You strongly prefer to stay silent. Output only SKIP or SAY: <text>.",
            "messages": [{"role": "user", "content": prompt}],
        });

        let url = format!("{}/v1/messages", self.config.base_url);
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            return None; // never let proactivity errors disrupt the user
        }

        let json: Value = resp.json().await.ok()?;
        let text = json
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|b| b.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        // Parse the decision. Default to silence on anything unexpected.
        if let Some(rest) = text.strip_prefix("SAY:") {
            let msg = rest.trim();
            if !msg.is_empty() && !msg.eq_ignore_ascii_case("skip") {
                return Some(msg.chars().take(160).collect());
            }
        }
        None
    }

    /// Log an interaction to today's log file
    fn log_interaction(&self, user_msg: &str, assistant_msg: &str) {
        let vault_path = self.config.vault_path_expanded();
        let logs_dir = vault_path.join("logs");
        let _ = std::fs::create_dir_all(&logs_dir);

        let today = Local::now().format("%Y-%m-%d").to_string();
        let timestamp = Local::now().format("%H:%M").to_string();
        let log_file = logs_dir.join(format!("{}.md", today));

        let entry = format!(
            "\n---\n{}\nUser: {}\nNebula: {}\n",
            timestamp,
            &user_msg[..user_msg.len().min(300)],
            &assistant_msg[..assistant_msg.len().min(800)],
        );

        let content = if log_file.exists() {
            std::fs::read_to_string(&log_file).unwrap_or_default() + &entry
        } else {
            format!("---\ntags: [log, {}]\ndate: {}\n---\n\nLog for {}\n{}", today, today, today, entry)
        };

        let _ = std::fs::write(&log_file, content);
    }

    /// Generate a session summary and save it to the vault
    pub fn save_session_summary(&self) {
        if self.history.len() < 4 {
            return; // Not enough conversation to summarize
        }

        let vault_path = self.config.vault_path_expanded();
        let summaries_dir = vault_path.join("summaries");
        let retro_dir = vault_path.join("retrospectives");
        let learnings_file = vault_path.join("memory/learnings.md");
        let _ = std::fs::create_dir_all(&summaries_dir);
        let _ = std::fs::create_dir_all(&retro_dir);
        let _ = std::fs::create_dir_all(vault_path.join("memory"));

        let today = Local::now().format("%Y-%m-%d").to_string();
        let timestamp = Local::now().format("%H%M").to_string();

        // Build conversation digest for retrospective
        let mut digest = Vec::new();
        for msg in &self.history {
            let role = if msg.role == "user" { "U" } else { "N" };
            let content = &msg.content;
            if content.len() > 10 && !content.starts_with('[') {
                digest.push(format!("{}: {}", role, &content[..content.len().min(200)]));
            }
        }
        digest.truncate(30);
        let conversation_digest = digest.join("\n");

        // Load existing learnings for context
        let existing_learnings = std::fs::read_to_string(&learnings_file).unwrap_or_default();

        // Run retrospective via LLM
        let retro_prompt = format!(
            "Analyze this Nebula session and extract learnings. Be concise.\n\n\
            Session ({} messages):\n{}\n\n\
            Existing learnings:\n{}\n\n\
            Respond with EXACTLY this format (no other text):\n\
            PREFERENCES: (user preferences/style noticed, 1-2 bullets or NONE)\n\
            PATTERNS: (recurring requests/workflows, 1-2 bullets or NONE)\n\
            IMPROVEMENTS: (what Nebula could do better, 1-2 bullets or NONE)\n\
            FACTS: (new facts learned about the user/environment, 1-2 bullets or NONE)",
            self.history.len(),
            &conversation_digest[..conversation_digest.len().min(3000)],
            &existing_learnings[..existing_learnings.len().min(1000)],
        );

        // Sync API call (non-streaming, fast model)
        let client = reqwest::blocking::Client::new();
        let body = serde_json::json!({
            "model": &self.config.fast_model,
            "max_tokens": 500,
            "system": "You analyze AI assistant sessions and extract actionable learnings. Be terse.",
            "messages": [{"role": "user", "content": retro_prompt}],
        });

        let url = format!("{}/v1/messages", self.config.base_url);
        let result = client.post(&url)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send();

        let retro_text = match result {
            Ok(resp) => {
                match resp.json::<serde_json::Value>() {
                    Ok(json) => {
                        json.get("content")
                            .and_then(|c| c.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|b| b.get("text"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string()
                    }
                    Err(_) => String::new(),
                }
            }
            Err(e) => {
                eprintln!("[retro] API error: {}", e);
                String::new()
            }
        };

        if retro_text.is_empty() {
            eprintln!("[retro] No response, skipping");
            return;
        }

        // Save full retrospective
        let retro_file = retro_dir.join(format!("{}-{}.md", today, timestamp));
        let retro_content = format!(
            "---\ntags: [retrospective, session]\ndate: {}\n---\n\n# Session Retrospective {}\n\nMessages: {}\n\n{}\n",
            today,
            Local::now().format("%Y-%m-%d %H:%M"),
            self.history.len(),
            retro_text,
        );
        let _ = std::fs::write(&retro_file, &retro_content);

        // Extract and append new learnings to the master learnings file
        let mut new_learnings = Vec::new();
        for line in retro_text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("- ") || trimmed.starts_with("• ") {
                let learning = trimmed.trim_start_matches("- ").trim_start_matches("• ").trim();
                if !learning.is_empty()
                    && learning != "NONE"
                    && !existing_learnings.contains(learning)
                {
                    new_learnings.push(format!("- {}", learning));
                }
            }
        }

        if !new_learnings.is_empty() {
            let mut content = existing_learnings;
            if content.is_empty() {
                content = format!("---\ntags: [learnings, memory]\nupdated: {}\n---\n\n# Nebula Learnings\n\nAccumulated from session retrospectives.\n\n", today);
            } else {
                // Update the date
                if let Some(pos) = content.find("updated: ") {
                    if let Some(end) = content[pos..].find('\n') {
                        content.replace_range(pos..pos + end, &format!("updated: {}", today));
                    }
                }
            }
            content.push_str(&format!("\n## {} {}\n{}\n",
                today,
                Local::now().format("%H:%M"),
                new_learnings.join("\n"),
            ));
            let _ = std::fs::write(&learnings_file, content);
            eprintln!("[retro] Saved {} new learnings", new_learnings.len());
        }

        // Also save basic summary
        let summary_file = summaries_dir.join(format!("{}-session-{}.md", today, timestamp));
        let mut topics: Vec<String> = Vec::new();
        for msg in &self.history {
            if msg.role == "user" {
                let content = &msg.content;
                if content.len() > 10 && !content.starts_with('[') {
                    topics.push(format!("- {}", &content[..content.len().min(100)]));
                }
            }
        }
        topics.truncate(10);
        let summary_content = format!(
            "---\ntags: [summary, session]\ndate: {}\n---\n\nSession Summary {}\n\nTopics:\n{}\n\nMessages: {}\n",
            today,
            Local::now().format("%Y-%m-%d %H:%M"),
            topics.join("\n"),
            self.history.len(),
        );

        let _ = std::fs::write(&summary_file, summary_content);
        eprintln!("Session summary saved: {}", summary_file.display());
    }
}

/// Classifies API failures so the fallback logic knows whether to retry the
/// same model, fall through to the next, abort entirely, or keep a partial.
#[derive(Debug)]
enum ApiError {
    /// Transient (429 / 5xx / network / empty) — retry same model, then fall back.
    Retryable {
        reason: String,
        retry_after: Option<std::time::Duration>,
    },
    /// Permanent (400 / 401 / 403) — abort the whole fallback chain.
    Fatal(String),
    /// Stream already delivered tokens before failing — keep what we have,
    /// retrying would double-print to the user.
    Streamed(String),
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Token(String),
    ToolStart { name: String, args: String },
    ToolResult { name: String, result: String },
    Done(String),
}
