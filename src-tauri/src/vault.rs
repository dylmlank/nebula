use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use walkdir::WalkDir;

/// A vault search result
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub path: String,
    pub header: String,
    pub text: String,
    pub score: f64,
}

/// A chunk of vault content
#[derive(Debug, Clone)]
struct Chunk {
    text: String,
    path: String,
    header: String,
    tokens: Vec<String>,
}

/// Index state — tracks file modification times
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct IndexState {
    files: HashMap<String, u64>, // path -> mtime as secs
}

/// The vault indexer — BM25 keyword search with header-based chunking.
/// Uses pure Rust for fast startup. Can be upgraded to vector search later.
pub struct VaultIndex {
    vault_path: PathBuf,
    chunks: Arc<RwLock<Vec<Chunk>>>,
    state_file: PathBuf,
}

impl VaultIndex {
    pub fn new(vault_path: PathBuf) -> Self {
        let state_file = vault_path.join(".index_state_v3.json");
        Self {
            vault_path,
            chunks: Arc::new(RwLock::new(Vec::new())),
            state_file,
        }
    }

    /// Index the vault — only processes changed files
    pub fn index(&self, force: bool) -> IndexStats {
        let mut state = self.load_state();
        let mut stats = IndexStats::default();
        let mut new_chunks = Vec::new();

        // On cold start, chunks are empty — must do full index
        let chunks_empty = self.chunks.read().map(|c| c.is_empty()).unwrap_or(true);
        let effective_force = force || chunks_empty;

        // Walk vault and process files
        let mut files_seen: HashMap<String, u64> = HashMap::new();

        for entry in WalkDir::new(&self.vault_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        {
            let path = entry.path();

            // Skip hidden directories
            if path
                .components()
                .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
            {
                // Allow the vault root itself
                if path != self.vault_path {
                    let rel = path.strip_prefix(&self.vault_path).unwrap_or(path);
                    if rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) {
                        continue;
                    }
                }
            }

            let rel_path = path
                .strip_prefix(&self.vault_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            files_seen.insert(rel_path.clone(), mtime);

            // Skip unchanged files (only if we have chunks in memory)
            if !effective_force {
                if let Some(&saved_mtime) = state.files.get(&rel_path) {
                    if saved_mtime == mtime {
                        // Keep existing chunks for this file
                        if let Ok(existing) = self.chunks.read() {
                            for chunk in existing.iter() {
                                if chunk.path == rel_path {
                                    new_chunks.push(chunk.clone());
                                }
                            }
                        }
                        stats.skipped += 1;
                        continue;
                    }
                }
            }

            // Read and chunk the file
            let content = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if content.trim().len() < 30 {
                continue;
            }

            let file_chunks = chunk_by_headers(&content, &rel_path);
            new_chunks.extend(file_chunks);
            stats.indexed += 1;
        }

        // Remove state entries for deleted files
        let deleted: Vec<String> = state
            .files
            .keys()
            .filter(|k| !files_seen.contains_key(*k))
            .cloned()
            .collect();
        stats.removed = deleted.len();

        // Update state
        state.files = files_seen;
        self.save_state(&state);

        stats.total_chunks = new_chunks.len();

        // Replace chunks
        if let Ok(mut chunks) = self.chunks.write() {
            *chunks = new_chunks;
        }

        stats
    }

    /// Hybrid search: BM25 keyword scoring with title/header boosting
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        if query.trim().is_empty() {
            return Vec::new();
        }

        let chunks = match self.chunks.read() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        if chunks.is_empty() {
            return Vec::new();
        }

        let query_terms = tokenize(query);
        if query_terms.is_empty() {
            return Vec::new();
        }

        // Calculate document frequency for IDF
        let doc_count = chunks.len() as f64;
        let mut doc_freq: HashMap<&str, usize> = HashMap::new();
        for chunk in chunks.iter() {
            let unique_terms: std::collections::HashSet<&str> =
                chunk.tokens.iter().map(|s| s.as_str()).collect();
            for term in &query_terms {
                if unique_terms.contains(term.as_str()) {
                    *doc_freq.entry(term.as_str()).or_insert(0) += 1;
                }
            }
        }

        // BM25 parameters
        let k1: f64 = 1.5;
        let b: f64 = 0.75;
        let avg_dl: f64 = chunks.iter().map(|c| c.tokens.len()).sum::<usize>() as f64 / doc_count;

        // Score each chunk
        let mut scored: Vec<(f64, usize)> = chunks
            .iter()
            .enumerate()
            .map(|(idx, chunk)| {
                let dl = chunk.tokens.len() as f64;
                let mut score = 0.0;

                // Count term frequencies in this chunk
                let mut term_freq: HashMap<&str, usize> = HashMap::new();
                for token in &chunk.tokens {
                    *term_freq.entry(token.as_str()).or_insert(0) += 1;
                }

                for term in &query_terms {
                    let tf = *term_freq.get(term.as_str()).unwrap_or(&0) as f64;
                    let df = *doc_freq.get(term.as_str()).unwrap_or(&0) as f64;

                    // IDF
                    let idf = ((doc_count - df + 0.5) / (df + 0.5) + 1.0).ln();

                    // BM25 TF normalization
                    let tf_norm = (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * dl / avg_dl));

                    score += idf * tf_norm;

                    // Boost for header/filename match
                    if chunk.header.to_lowercase().contains(term) {
                        score += idf * 0.5;
                    }
                    if chunk.path.to_lowercase().contains(term) {
                        score += idf * 0.3;
                    }
                }

                (score, idx)
            })
            .filter(|(score, _)| *score > 0.0)
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Deduplicate by path+header and take top N
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut results = Vec::new();

        for (score, idx) in scored {
            let chunk = &chunks[idx];
            let key = format!("{}:{}", chunk.path, chunk.header);
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);

            // Normalize score to ~0-1 range
            let normalized_score = (score / (query_terms.len() as f64 * 3.0)).min(1.0);

            results.push(SearchResult {
                path: chunk.path.clone(),
                header: chunk.header.clone(),
                text: chunk.text.clone(),
                score: normalized_score,
            });

            if results.len() >= limit {
                break;
            }
        }

        results
    }

    /// Get memory context for system prompt
    pub fn get_context(&self) -> String {
        let mut parts = Vec::new();

        // Core facts
        let facts_path = self.vault_path.join("memory/facts.md");
        if let Ok(facts) = fs::read_to_string(&facts_path) {
            let facts = facts.trim();
            if !facts.is_empty() {
                parts.push(format!("Key facts:\n{}", facts));
            }
        }

        // Current context
        let ctx_path = self.vault_path.join("memory/context.md");
        if let Ok(ctx) = fs::read_to_string(&ctx_path) {
            let ctx = ctx.trim();
            if !ctx.is_empty() {
                parts.push(format!("Focus: {}", ctx));
            }
        }

        parts.join("\n\n")
    }

    /// Recall relevant memories for a topic
    pub fn recall(&self, topic: &str, limit: usize) -> String {
        let results = self.search(topic, limit);
        if results.is_empty() {
            return String::new();
        }

        results
            .iter()
            .filter(|r| r.score > 0.1)
            .map(|r| {
                let text = if r.text.starts_with('[') {
                    r.text.find('\n').map(|i| &r.text[i + 1..]).unwrap_or(&r.text)
                } else {
                    &r.text
                };
                format!("[{}] {}", r.path, &text[..text.len().min(200)])
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn load_state(&self) -> IndexState {
        if self.state_file.exists() {
            fs::read_to_string(&self.state_file)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            IndexState::default()
        }
    }

    fn save_state(&self, state: &IndexState) {
        if let Ok(json) = serde_json::to_string(state) {
            let _ = fs::write(&self.state_file, json);
        }
    }

    /// Start watching the vault for file changes and auto-reindex
    pub fn start_watcher(self: &Arc<Self>) {
        let vault_path = self.vault_path.clone();
        let index = self.clone();

        std::thread::spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();

            let mut watcher = match RecommendedWatcher::new(
                move |res: Result<Event, notify::Error>| {
                    if let Ok(event) = res {
                        let _ = tx.send(event);
                    }
                },
                notify::Config::default(),
            ) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("File watcher failed: {}", e);
                    return;
                }
            };

            if let Err(e) = watcher.watch(&vault_path, RecursiveMode::Recursive) {
                eprintln!("Watch failed: {}", e);
                return;
            }

            eprintln!("Vault watcher active: {}", vault_path.display());

            // Debounce: collect events for 2 seconds then reindex
            let mut last_reindex = std::time::Instant::now();

            loop {
                match rx.recv_timeout(std::time::Duration::from_secs(2)) {
                    Ok(event) => {
                        // Only care about file modifications/creates/deletes
                        match event.kind {
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                                // Check if it's a .md file
                                let is_md = event.paths.iter().any(|p| {
                                    p.extension().map(|e| e == "md").unwrap_or(false)
                                });
                                if is_md && last_reindex.elapsed() > std::time::Duration::from_secs(3) {
                                    let stats = index.index(false);
                                    if stats.indexed > 0 {
                                        eprintln!("Auto-reindex: {} files updated, {} chunks", stats.indexed, stats.total_chunks);
                                    }
                                    last_reindex = std::time::Instant::now();
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    }
}

/// Chunk markdown content by headers
fn chunk_by_headers(content: &str, file_path: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = content.lines().collect();
    let mut chunks = Vec::new();

    // Skip frontmatter
    let mut body_start = 0;
    if lines.first() == Some(&"---") {
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.trim() == "---" {
                body_start = i + 1;
                break;
            }
        }
    }

    // Parse into sections by header
    let header_re = Regex::new(r"^(#{1,4})\s+(.+)").unwrap();
    let mut sections: Vec<(String, usize, Vec<&str>)> = Vec::new(); // (header, level, lines)
    let mut current_header = file_path
        .split('/')
        .last()
        .unwrap_or(file_path)
        .trim_end_matches(".md")
        .to_string();
    let mut current_level = 0;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in lines.iter().skip(body_start) {
        if let Some(cap) = header_re.captures(line) {
            // Save current section
            if !current_lines.is_empty() {
                let text = current_lines.join("\n").trim().to_string();
                if text.len() > 20 {
                    sections.push((current_header.clone(), current_level, current_lines.clone()));
                }
            }
            current_header = cap[2].trim().to_string();
            current_level = cap[1].len();
            current_lines = Vec::new();
        } else {
            current_lines.push(line);
        }
    }

    // Last section
    if !current_lines.is_empty() {
        let text = current_lines.join("\n").trim().to_string();
        if text.len() > 20 {
            sections.push((current_header, current_level, current_lines));
        }
    }

    // If no sections, treat whole file as one chunk
    if sections.is_empty() {
        let text = lines[body_start..].join("\n").trim().to_string();
        if text.len() > 20 {
            let chunk_text = format!("[{}] {}\n{}", file_path, file_path, text);
            let tokens = tokenize(&chunk_text);
            chunks.push(Chunk {
                text: chunk_text,
                path: file_path.to_string(),
                header: file_path.to_string(),
                tokens,
            });
        }
        return chunks;
    }

    // Build chunks with context
    for (header, _level, section_lines) in sections {
        let section_text = section_lines.join("\n").trim().to_string();
        let chunk_text = format!("[{}] {}\n{}", file_path, header, section_text);

        // Split oversized chunks
        if chunk_text.len() > 1500 {
            let sub_chunks = split_large_chunk(&chunk_text, 1200);
            for sub in sub_chunks {
                let tokens = tokenize(&sub);
                chunks.push(Chunk {
                    text: sub,
                    path: file_path.to_string(),
                    header: header.clone(),
                    tokens,
                });
            }
        } else {
            let tokens = tokenize(&chunk_text);
            chunks.push(Chunk {
                text: chunk_text,
                path: file_path.to_string(),
                header: header.clone(),
                tokens,
            });
        }
    }

    chunks
}

/// Split a large chunk at paragraph boundaries
fn split_large_chunk(text: &str, max_size: usize) -> Vec<String> {
    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    let mut sub_chunks = Vec::new();
    let mut current = Vec::new();
    let mut current_len = 0;

    for para in paragraphs {
        if current_len + para.len() > max_size && !current.is_empty() {
            sub_chunks.push(current.join("\n\n"));
            current = vec![para];
            current_len = para.len();
        } else {
            current.push(para);
            current_len += para.len();
        }
    }

    if !current.is_empty() {
        sub_chunks.push(current.join("\n\n"));
    }

    sub_chunks
}

/// Tokenize text for BM25 scoring
fn tokenize(text: &str) -> Vec<String> {
    let re = Regex::new(r"\w+").unwrap();
    re.find_iter(&text.to_lowercase())
        .map(|m| m.as_str().to_string())
        .collect()
}

#[derive(Debug, Default)]
pub struct IndexStats {
    pub indexed: usize,
    pub skipped: usize,
    pub removed: usize,
    pub total_chunks: usize,
}
