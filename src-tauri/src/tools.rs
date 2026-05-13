use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{Local, Utc};

/// Parse [TOOL:name key=value] calls from LLM output
pub fn parse_tool_calls(text: &str) -> Vec<(String, HashMap<String, String>)> {
    let re = Regex::new(r"\[TOOL:(\w+)\s*(.*?)\]").unwrap();
    let mut calls = Vec::new();

    for cap in re.captures_iter(text) {
        let name = cap[1].to_string();
        let args_str = cap[2].trim().to_string();
        let kwargs = parse_args(&args_str);
        calls.push((name, kwargs));
    }

    calls
}

/// Parse key=value arguments
fn parse_args(args_str: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();
    if args_str.is_empty() {
        return result;
    }

    // Find all key= positions
    let key_re = Regex::new(r"(?:^|\s)(\w+)=").unwrap();
    let positions: Vec<(usize, String)> = key_re
        .captures_iter(args_str)
        .map(|cap| {
            let m = cap.get(1).unwrap();
            (cap.get(0).unwrap().end(), m.as_str().to_string())
        })
        .collect();

    if positions.is_empty() {
        result.insert("command".into(), args_str.to_string());
        return result;
    }

    for (i, (val_start, key)) in positions.iter().enumerate() {
        let val_end = if i + 1 < positions.len() {
            // Find where next key starts (before the space+key=)
            let next_match = key_re.find_at(args_str, *val_start).unwrap();
            next_match.start()
        } else {
            args_str.len()
        };

        let mut value = args_str[*val_start..val_end].trim().to_string();

        // Strip surrounding quotes
        if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
            value = value[1..value.len() - 1].to_string();
        }

        result.insert(key.clone(), value);
    }

    result
}

/// Strip tool tokens from text (for display/TTS)
pub fn strip_tool_tokens(text: &str) -> String {
    let tool_re = Regex::new(r"\[TOOL:\w+.*?\]").unwrap();
    let create_re = Regex::new(r"(?s)\[CREATE_TOOL:\w+\].*?\[/CREATE_TOOL\]").unwrap();

    let text = tool_re.replace_all(text, "");
    let text = create_re.replace_all(&text, "");

    // Remove tool execution output lines
    text.lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with("⚙️")
                && !trimmed.starts_with("→")
                && !trimmed.starts_with("⚡")
                && !(trimmed.starts_with('[') && trimmed.contains("]:"))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

// ─── Built-in Tool Implementations ───

pub fn run_command(args: &HashMap<String, String>) -> String {
    let command = args
        .get("command")
        .or_else(|| args.get("cmd"))
        .cloned()
        .unwrap_or_default();

    if command.is_empty() {
        return "Error: no command".into();
    }

    let _timeout_secs: u64 = args
        .get("timeout")
        .and_then(|t| t.parse().ok())
        .unwrap_or(30);

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));

    match Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(&home)
        .output()
    {
        Ok(output) => {
            let mut out = String::from_utf8_lossy(&output.stdout).to_string();
            let err = String::from_utf8_lossy(&output.stderr);
            if !err.is_empty() {
                out.push('\n');
                out.push_str(&err);
            }
            if !output.status.success() {
                out.push_str(&format!("\n(exit {})", output.status.code().unwrap_or(-1)));
            }
            let out = out.trim().to_string();
            if out.is_empty() {
                "(no output)".into()
            } else if out.len() > 3000 {
                format!("{}...(truncated)", &out[..3000])
            } else {
                out
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn read_file(args: &HashMap<String, String>) -> String {
    let path = args.get("path").cloned().unwrap_or_default();
    if path.is_empty() {
        return "Error: no path".into();
    }

    let expanded = shellexpand::tilde(&path);
    let p = Path::new(expanded.as_ref());

    if !p.is_file() {
        return format!("Not found: {}", p.display());
    }

    match fs::read_to_string(p) {
        Ok(content) => {
            if content.len() > 8000 {
                format!("{}...(truncated)", &content[..8000])
            } else {
                content
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn write_file(args: &HashMap<String, String>) -> String {
    let path = args.get("path").cloned().unwrap_or_default();
    let content = args.get("content").cloned().unwrap_or_default();

    if path.is_empty() {
        return "Error: no path".into();
    }

    let expanded = shellexpand::tilde(&path);
    let p = Path::new(expanded.as_ref());

    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }

    match fs::write(p, &content) {
        Ok(_) => format!("Wrote {} chars to {}", content.len(), p.display()),
        Err(e) => format!("Error: {}", e),
    }
}

pub fn list_files(args: &HashMap<String, String>) -> String {
    let path = args.get("path").cloned().unwrap_or_else(|| ".".into());
    let _pattern = args.get("pattern").cloned().unwrap_or_else(|| "*".into());

    let expanded = shellexpand::tilde(&path);
    let p = Path::new(expanded.as_ref());

    if !p.is_dir() {
        return format!("Not a directory: {}", p.display());
    }

    match fs::read_dir(p) {
        Ok(entries) => {
            let mut items: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    if is_dir {
                        format!("  📁 {}", name)
                    } else {
                        format!("     {}", name)
                    }
                })
                .collect();
            items.sort();
            items.truncate(80);

            if items.is_empty() {
                format!("Empty: {}", p.display())
            } else {
                format!("{}/\n{}", p.display(), items.join("\n"))
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn web_search(args: &HashMap<String, String>) -> String {
    let query = args.get("query").cloned().unwrap_or_default();
    if query.is_empty() {
        return "Error: no query".into();
    }

    // Use ddgr or curl to DuckDuckGo
    let result = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "curl -s 'https://html.duckduckgo.com/html/?q={}' | grep -oP '<a rel=\"nofollow\" class=\"result__a\" href=\"[^\"]+\">[^<]+' | head -5 | sed 's/<a[^>]*>//' | sed 's/<\\/a>//'",
            urlencoding::encode(&query)
        ))
        .output();

    match result {
        Ok(output) => {
            let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if out.is_empty() {
                format!("No results for: {}", query)
            } else {
                out
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn web_fetch(args: &HashMap<String, String>) -> String {
    let url = args.get("url").cloned().unwrap_or_default();
    if url.is_empty() {
        return "Error: no url".into();
    }

    let result = Command::new("curl")
        .args(["-sL", "-m", "10", "-A", "Nebula/3.0", &url])
        .output();

    match result {
        Ok(output) => {
            let out = String::from_utf8_lossy(&output.stdout).to_string();
            if out.len() > 8000 {
                format!("{}...(truncated)", &out[..8000])
            } else if out.is_empty() {
                "Error: empty response".into()
            } else {
                out
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn clipboard(args: &HashMap<String, String>) -> String {
    let action = args.get("action").cloned().unwrap_or_else(|| "get".into());
    let content = args.get("content").cloned().unwrap_or_default();

    if action == "set" {
        let result = Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(stdin) = child.stdin.as_mut() {
                    stdin.write_all(content.as_bytes())?;
                }
                child.wait()
            });

        match result {
            Ok(_) => format!("Copied {} chars", content.len()),
            Err(_) => {
                // Fallback to xclip
                match Command::new("xclip")
                    .args(["-selection", "clipboard"])
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                    .and_then(|mut child| {
                        use std::io::Write;
                        if let Some(stdin) = child.stdin.as_mut() {
                            stdin.write_all(content.as_bytes())?;
                        }
                        child.wait()
                    }) {
                    Ok(_) => format!("Copied {} chars", content.len()),
                    Err(e) => format!("Error: {}", e),
                }
            }
        }
    } else {
        match Command::new("wl-paste").output() {
            Ok(output) if output.status.success() => {
                let out = String::from_utf8_lossy(&output.stdout).to_string();
                if out.len() > 3000 {
                    format!("{}...(truncated)", &out[..3000])
                } else if out.is_empty() {
                    "(empty)".into()
                } else {
                    out
                }
            }
            _ => match Command::new("xclip")
                .args(["-selection", "clipboard", "-o"])
                .output()
            {
                Ok(output) => String::from_utf8_lossy(&output.stdout)
                    .to_string()
                    .chars()
                    .take(3000)
                    .collect(),
                Err(e) => format!("Error: {}", e),
            },
        }
    }
}

pub fn notify(args: &HashMap<String, String>) -> String {
    let title = args.get("title").cloned().unwrap_or_else(|| "Nebula".into());
    let message = args.get("message").cloned().unwrap_or_default();

    if message.is_empty() {
        return "Error: no message".into();
    }

    match Command::new("notify-send").args([&title, &message]).output() {
        Ok(_) => format!("Notified: {}", title),
        Err(e) => format!("Error: {}", e),
    }
}

pub fn open_url(args: &HashMap<String, String>) -> String {
    let url = args.get("url").cloned().unwrap_or_default();
    if url.is_empty() {
        return "Error: no url".into();
    }

    let _ = Command::new("xdg-open")
        .arg(&url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    format!("Opened: {}", url)
}

pub fn get_datetime(_args: &HashMap<String, String>) -> String {
    Local::now().format("%A, %B %d, %Y at %I:%M %p").to_string()
}

pub fn calc(args: &HashMap<String, String>) -> String {
    let expr = args.get("expr").cloned().unwrap_or_default();
    if expr.is_empty() {
        return "Error: no expression".into();
    }

    // Use bc for calculation
    match Command::new("sh")
        .arg("-c")
        .arg(format!("echo '{}' | bc -l 2>&1", expr))
        .output()
    {
        Ok(output) => {
            let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if out.is_empty() {
                "Error: invalid expression".into()
            } else {
                out
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn system_info(_args: &HashMap<String, String>) -> String {
    let mut lines = Vec::new();

    // OS info
    if let Ok(release) = fs::read_to_string("/etc/os-release") {
        for line in release.lines() {
            if line.starts_with("PRETTY_NAME=") {
                lines.push(format!("OS: {}", line[13..].trim_matches('"')));
                break;
            }
        }
    }

    // Hostname
    if let Ok(hostname) = fs::read_to_string("/etc/hostname") {
        lines.push(format!("Host: {}", hostname.trim()));
    }

    // Memory
    if let Ok(meminfo) = fs::read_to_string("/proc/meminfo") {
        for line in meminfo.lines() {
            if line.starts_with("MemAvailable:") {
                let kb: u64 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                lines.push(format!("RAM free: {}M", kb / 1024));
                break;
            }
        }
    }

    // Disk
    let output = Command::new("df").args(["-h", "/"]).output();
    if let Ok(output) = output {
        let out = String::from_utf8_lossy(&output.stdout);
        if let Some(line) = out.lines().nth(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                lines.push(format!("Disk: {} used / {} total", parts[2], parts[1]));
            }
        }
    }

    lines.join("\n")
}

pub fn remember(args: &HashMap<String, String>) -> String {
    let fact = args.get("fact").cloned().unwrap_or_default();
    if fact.is_empty() {
        return "Error: no fact".into();
    }

    let vault_path = dirs::home_dir()
        .unwrap_or_default()
        .join("Documents/vault/JARVIS/memory/facts.md");

    let existing = fs::read_to_string(&vault_path).unwrap_or_default();
    let lines: Vec<&str> = existing.lines().filter(|l| !l.trim().is_empty()).collect();

    // Don't duplicate
    let fact_trimmed = fact.trim();
    for line in &lines {
        if line.trim_start_matches("- ").trim() == fact_trimmed {
            return format!("Already known: {}", fact_trimmed);
        }
    }

    let mut new_content = existing.clone();
    new_content.push_str(&format!("- {}\n", fact_trimmed));

    if let Some(parent) = vault_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&vault_path, new_content);

    format!("Remembered: {}", fact_trimmed)
}

pub fn knowledge_search(args: &HashMap<String, String>) -> String {
    let query = args.get("query").cloned().unwrap_or_default();
    if query.is_empty() {
        return "Error: no query".into();
    }

    // Simple grep-based search across vault
    let vault_path = dirs::home_dir()
        .unwrap_or_default()
        .join("Documents/vault/JARVIS");

    let result = Command::new("grep")
        .args(["-ril", "--include=*.md", &query, vault_path.to_str().unwrap_or(".")])
        .output();

    match result {
        Ok(output) => {
            let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if out.is_empty() {
                format!("No results for: {}", query)
            } else {
                // Read snippets from matching files
                let files: Vec<&str> = out.lines().take(5).collect();
                let mut results = Vec::new();
                for file in files {
                    if let Ok(content) = fs::read_to_string(file) {
                        let snippet: String = content.lines()
                            .filter(|l| l.to_lowercase().contains(&query.to_lowercase()))
                            .take(2)
                            .collect::<Vec<_>>()
                            .join("\n");
                        let rel = file.replace(vault_path.to_str().unwrap_or(""), "");
                        results.push(format!("[{}]\n{}", rel, snippet));
                    }
                }
                results.join("\n\n")
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn knowledge_save(args: &HashMap<String, String>) -> String {
    let title = args.get("title").cloned().unwrap_or_default();
    let content = args.get("content").cloned().unwrap_or_default();
    let category = args.get("category").cloned().unwrap_or_else(|| "knowledge".into());
    let tags = args.get("tags").cloned().unwrap_or_else(|| category.clone());

    if title.is_empty() || content.is_empty() {
        return "Error: need title and content".into();
    }

    let vault_path = dirs::home_dir()
        .unwrap_or_default()
        .join("Documents/vault/JARVIS")
        .join(&category);

    let _ = fs::create_dir_all(&vault_path);

    // Sanitize filename
    let safe_title: String = title
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .take(60)
        .collect();
    let safe_title = safe_title.trim().replace(' ', "-").to_lowercase();

    let file_path = vault_path.join(format!("{}.md", safe_title));
    let today = Local::now().format("%Y-%m-%d").to_string();

    let note = format!(
        "---\ntags: [{}]\ndate: {}\n---\n\n{}\n\n{}\n",
        tags, today, title, content
    );

    match fs::write(&file_path, note) {
        Ok(_) => format!("Saved: {}", file_path.display()),
        Err(e) => format!("Error: {}", e),
    }
}

pub fn save_decision(args: &HashMap<String, String>) -> String {
    let title = args.get("title").cloned().unwrap_or_default();
    let reasoning = args.get("reasoning").cloned().unwrap_or_default();
    let outcome = args.get("outcome").cloned().unwrap_or_default();

    if title.is_empty() || reasoning.is_empty() {
        return "Error: need title and reasoning".into();
    }

    let content = if outcome.is_empty() {
        format!("Reasoning: {}", reasoning)
    } else {
        format!("Reasoning: {}\n\nOutcome: {}", reasoning, outcome)
    };

    let mut save_args = HashMap::new();
    save_args.insert("title".into(), title.clone());
    save_args.insert("content".into(), content);
    save_args.insert("category".into(), "decisions".into());
    save_args.insert("tags".into(), "decision".into());
    knowledge_save(&save_args)
}

pub fn save_idea(args: &HashMap<String, String>) -> String {
    let title = args.get("title").cloned().unwrap_or_default();
    let description = args.get("description").cloned().unwrap_or_default();

    if title.is_empty() || description.is_empty() {
        return "Error: need title and description".into();
    }

    let mut save_args = HashMap::new();
    save_args.insert("title".into(), title);
    save_args.insert("content".into(), description);
    save_args.insert("category".into(), "ideas".into());
    save_args.insert("tags".into(), "idea".into());
    knowledge_save(&save_args)
}

pub fn save_learning(args: &HashMap<String, String>) -> String {
    let title = args.get("title").cloned().unwrap_or_default();
    let lesson = args.get("lesson").cloned().unwrap_or_default();
    let source = args.get("source").cloned().unwrap_or_default();

    if title.is_empty() || lesson.is_empty() {
        return "Error: need title and lesson".into();
    }

    let content = if source.is_empty() {
        lesson
    } else {
        format!("{}\n\nSource: {}", lesson, source)
    };

    let mut save_args = HashMap::new();
    save_args.insert("title".into(), title);
    save_args.insert("content".into(), content);
    save_args.insert("category".into(), "learnings".into());
    save_args.insert("tags".into(), "learning".into());
    knowledge_save(&save_args)
}

pub fn save_person(args: &HashMap<String, String>) -> String {
    let name = args.get("name").cloned().unwrap_or_default();
    let info = args.get("info").cloned().unwrap_or_default();

    if name.is_empty() || info.is_empty() {
        return "Error: need name and info".into();
    }

    let vault_path = dirs::home_dir()
        .unwrap_or_default()
        .join("Documents/vault/JARVIS/people");
    let _ = fs::create_dir_all(&vault_path);

    let filename = name.to_lowercase().replace(' ', "-");
    let file_path = vault_path.join(format!("{}.md", filename));
    let today = Local::now().format("%Y-%m-%d").to_string();

    let content = if file_path.exists() {
        let existing = fs::read_to_string(&file_path).unwrap_or_default();
        format!("{}\n\n{}: {}\n", existing, today, info)
    } else {
        format!("---\ntags: [person]\ncreated: {}\n---\n\n{}\n\n{}\n", today, name, info)
    };

    match fs::write(&file_path, content) {
        Ok(_) => format!("Person saved: {}", name),
        Err(e) => format!("Error: {}", e),
    }
}

pub fn recall(args: &HashMap<String, String>) -> String {
    let topic = args.get("topic").cloned().unwrap_or_default();
    if topic.is_empty() {
        return "Error: no topic".into();
    }
    // Delegate to knowledge_search
    let mut search_args = HashMap::new();
    search_args.insert("query".into(), topic);
    knowledge_search(&search_args)
}

pub fn daily_summary(_args: &HashMap<String, String>) -> String {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let log_path = dirs::home_dir()
        .unwrap_or_default()
        .join(format!("Documents/vault/JARVIS/logs/{}.md", today));

    if !log_path.exists() {
        return "No log for today.".into();
    }

    let content = fs::read_to_string(&log_path).unwrap_or_default();
    let interaction_count = content.matches("\n---\n").count();

    format!("Today: {} interactions logged", interaction_count)
}

pub fn pip_install(args: &HashMap<String, String>) -> String {
    let package = args.get("package").cloned().unwrap_or_default();
    if package.is_empty() {
        return "Error: no package".into();
    }

    match Command::new("pip")
        .args(["install", "--user", &package])
        .output()
    {
        Ok(output) => {
            if output.status.success() {
                format!("Installed {}", package)
            } else {
                format!("Error: {}", String::from_utf8_lossy(&output.stderr).chars().take(300).collect::<String>())
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn git(args: &HashMap<String, String>) -> String {
    let path = args.get("path").cloned().unwrap_or_else(|| ".".into());
    let command = args.get("command").cloned().unwrap_or_default();

    if command.is_empty() {
        return "Error: no command".into();
    }

    let expanded = shellexpand::tilde(&path);

    match Command::new("sh")
        .arg("-c")
        .arg(format!("git {}", command))
        .current_dir(expanded.as_ref())
        .output()
    {
        Ok(output) => {
            let mut out = String::from_utf8_lossy(&output.stdout).to_string();
            let err = String::from_utf8_lossy(&output.stderr);
            if !err.is_empty() {
                out.push_str(&err);
            }
            let out = out.trim().to_string();
            if out.is_empty() {
                "(no output)".into()
            } else if out.len() > 2000 {
                format!("{}...(truncated)", &out[..2000])
            } else {
                out
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn multi_command(args: &HashMap<String, String>) -> String {
    let commands = args.get("commands").cloned().unwrap_or_default();
    if commands.is_empty() {
        return "Error: no commands".into();
    }

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let cmd_list: Vec<&str> = commands.split(|c| c == ';' || c == '\n')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .take(10)
        .collect();

    let mut results = Vec::new();
    for cmd in cmd_list {
        match Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&home)
            .output()
        {
            Ok(output) => {
                let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if out.is_empty() {
                    results.push(format!("$ {}\n(ok)", cmd));
                } else {
                    results.push(format!("$ {}\n{}", cmd, &out[..out.len().min(500)]));
                }
            }
            Err(e) => results.push(format!("$ {}\nError: {}", cmd, e)),
        }
    }

    results.join("\n\n")
}

pub fn find_files(args: &HashMap<String, String>) -> String {
    let path = args.get("path").cloned().unwrap_or_else(|| ".".into());
    let name = args.get("name").or_else(|| args.get("pattern")).cloned().unwrap_or_default();
    let content = args.get("content").cloned().unwrap_or_default();

    let expanded = shellexpand::tilde(&path);

    if !content.is_empty() {
        // Grep for content
        match Command::new("grep")
            .args(["-rl", "--include=*.py", "--include=*.js", "--include=*.ts",
                   "--include=*.md", "--include=*.json", "--include=*.toml",
                   &content, expanded.as_ref()])
            .output()
        {
            Ok(output) => {
                let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if out.is_empty() { "No matches".into() } else { out.lines().take(20).collect::<Vec<_>>().join("\n") }
            }
            Err(e) => format!("Error: {}", e),
        }
    } else if !name.is_empty() {
        match Command::new("find")
            .args([expanded.as_ref(), "-name", &name, "-type", "f"])
            .output()
        {
            Ok(output) => {
                let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if out.is_empty() { "No matches".into() } else { out.lines().take(30).collect::<Vec<_>>().join("\n") }
            }
            Err(e) => format!("Error: {}", e),
        }
    } else {
        "Error: provide name (glob pattern) or content (search string)".into()
    }
}

pub fn replace_in_file(args: &HashMap<String, String>) -> String {
    let path = args.get("path").cloned().unwrap_or_default();
    let find = args.get("find").cloned().unwrap_or_default();
    let replace = args.get("replace").cloned().unwrap_or_default();

    if path.is_empty() || find.is_empty() {
        return "Error: need path and find".into();
    }

    let expanded = shellexpand::tilde(&path);
    let p = Path::new(expanded.as_ref());

    if !p.is_file() {
        return format!("Not found: {}", p.display());
    }

    match fs::read_to_string(p) {
        Ok(text) => {
            let count = text.matches(&find).count();
            if count == 0 {
                return format!("'{}' not found in {}", &find[..find.len().min(40)], p.display());
            }
            let new_text = text.replace(&find, &replace);
            match fs::write(p, new_text) {
                Ok(_) => format!("Replaced {} occurrence(s) in {}", count, p.display()),
                Err(e) => format!("Error writing: {}", e),
            }
        }
        Err(e) => format!("Error reading: {}", e),
    }
}

pub fn processes(args: &HashMap<String, String>) -> String {
    let filter = args.get("filter").cloned().unwrap_or_default();

    let cmd = if filter.is_empty() {
        "ps aux --sort=-%mem | head -15".to_string()
    } else {
        format!("ps aux | grep -i '{}' | grep -v grep | head -15", filter)
    };

    match Command::new("sh").arg("-c").arg(&cmd).output() {
        Ok(output) => {
            let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if out.is_empty() { "(none)".into() } else { out[..out.len().min(2000)].to_string() }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn kill_process(args: &HashMap<String, String>) -> String {
    let target = args.get("target").cloned().unwrap_or_default();
    if target.is_empty() {
        return "Error: no target".into();
    }

    if target.chars().all(|c| c.is_ascii_digit()) {
        let _ = Command::new("kill").arg(&target).output();
        format!("Killed PID {}", target)
    } else {
        match Command::new("pkill").args(["-f", &target]).output() {
            Ok(output) => {
                if output.status.success() {
                    format!("Killed '{}'", target)
                } else {
                    format!("No process: {}", target)
                }
            }
            Err(e) => format!("Error: {}", e),
        }
    }
}

pub fn set_context(args: &HashMap<String, String>) -> String {
    let context = args.get("context").cloned().unwrap_or_default();
    if context.is_empty() {
        return "Error: no context".into();
    }

    let ctx_file = dirs::home_dir()
        .unwrap_or_default()
        .join("Documents/vault/JARVIS/memory/context.md");

    if let Some(parent) = ctx_file.parent() {
        let _ = fs::create_dir_all(parent);
    }

    match fs::write(&ctx_file, format!("{}\n", context.trim())) {
        Ok(_) => format!("Context updated: {}", &context[..context.len().min(80)]),
        Err(e) => format!("Error: {}", e),
    }
}

// ─── Google Integration Tools ───

pub fn google_search(args: &HashMap<String, String>) -> String {
    let query = args.get("query").cloned().unwrap_or_default();
    if query.is_empty() {
        return "Error: no query".into();
    }

    // Try Google Custom Search API first (if configured)
    let config_path = dirs::config_dir()
        .unwrap_or_default()
        .join("nebula/config.json");

    if let Ok(content) = fs::read_to_string(&config_path) {
        if let Ok(config) = serde_json::from_str::<serde_json::Value>(&content) {
            let api_key = config.get("google")
                .and_then(|g| g.get("search_api_key"))
                .and_then(|k| k.as_str())
                .unwrap_or("");
            let cx = config.get("google")
                .and_then(|g| g.get("search_cx"))
                .and_then(|c| c.as_str())
                .unwrap_or("");

            if !api_key.is_empty() && !cx.is_empty() {
                let url = format!(
                    "https://www.googleapis.com/customsearch/v1?key={}&cx={}&q={}&num=5",
                    api_key, cx, urlencoding::encode(&query)
                );

                match Command::new("curl")
                    .args(["-s", "-m", "10", &url])
                    .output()
                {
                    Ok(output) => {
                        let body = String::from_utf8_lossy(&output.stdout);
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                            if let Some(items) = json.get("items").and_then(|i| i.as_array()) {
                                let results: Vec<String> = items.iter().take(5).map(|item| {
                                    let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("");
                                    let link = item.get("link").and_then(|l| l.as_str()).unwrap_or("");
                                    let snippet = item.get("snippet").and_then(|s| s.as_str()).unwrap_or("");
                                    format!("• {}\n  {}\n  {}", title, link, snippet)
                                }).collect();
                                return results.join("\n\n");
                            }
                        }
                    }
                    Err(_) => {}
                }
            }
        }
    }

    // Fallback to DuckDuckGo
    web_search(args)
}

pub fn calendar(args: &HashMap<String, String>) -> String {
    let action = args.get("action").cloned().unwrap_or_else(|| "list".into());

    // Load token
    let token_path = dirs::config_dir()
        .unwrap_or_default()
        .join("nebula/google_token.json");

    let access_token = match fs::read_to_string(&token_path) {
        Ok(content) => {
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(token_data) => {
                    // Check if we need to refresh
                    let access = token_data.get("access_token")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    if access.is_empty() {
                        return "Not authenticated. Ask the user to run: calendar_auth action=start".into();
                    }
                    access
                }
                Err(_) => return "Token file corrupted. Re-authenticate with calendar_auth".into(),
            }
        }
        Err(_) => return "Not authenticated with Google. Ask the user to set up Google Calendar in Settings, then use calendar_auth action=start".into(),
    };

    match action.as_str() {
        "list" => {
            let now = chrono::Utc::now().to_rfc3339();
            let url = format!(
                "https://www.googleapis.com/calendar/v3/calendars/primary/events?maxResults=10&orderBy=startTime&singleEvents=true&timeMin={}",
                urlencoding::encode(&now)
            );

            match Command::new("curl")
                .args(["-s", "-m", "10",
                       "-H", &format!("Authorization: Bearer {}", access_token),
                       &url])
                .output()
            {
                Ok(output) => {
                    let body = String::from_utf8_lossy(&output.stdout);
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                        if let Some(error) = json.get("error") {
                            return format!("API Error: {}. May need to re-authenticate.", error);
                        }
                        if let Some(items) = json.get("items").and_then(|i| i.as_array()) {
                            if items.is_empty() {
                                return "No upcoming events.".into();
                            }
                            let events: Vec<String> = items.iter().map(|item| {
                                let summary = item.get("summary").and_then(|s| s.as_str()).unwrap_or("(no title)");
                                let start = item.get("start")
                                    .and_then(|s| s.get("dateTime").or_else(|| s.get("date")))
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("unknown");
                                format!("• {} — {}", start, summary)
                            }).collect();
                            return format!("Upcoming events:\n{}", events.join("\n"));
                        }
                    }
                    format!("Could not parse response: {}", &body[..body.len().min(200)])
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "create" => {
            let title = args.get("title").cloned().unwrap_or_else(|| "New Event".into());
            let date = args.get("date").cloned().unwrap_or_default();
            let time = args.get("time").cloned().unwrap_or_else(|| "09:00".into());
            let duration = args.get("duration").and_then(|d| d.parse::<u32>().ok()).unwrap_or(60);

            if date.is_empty() {
                return "Error: need date (YYYY-MM-DD format)".into();
            }

            let start = format!("{}T{}:00", date, time);
            let end_hour = time.split(':').next().and_then(|h| h.parse::<u32>().ok()).unwrap_or(9) + duration / 60;
            let end_min = time.split(':').nth(1).and_then(|m| m.parse::<u32>().ok()).unwrap_or(0) + duration % 60;
            let end = format!("{}T{:02}:{:02}:00", date, end_hour, end_min);

            let event_body = serde_json::json!({
                "summary": title,
                "start": {"dateTime": start, "timeZone": "local"},
                "end": {"dateTime": end, "timeZone": "local"},
            });

            match Command::new("curl")
                .args(["-s", "-m", "10", "-X", "POST",
                       "-H", &format!("Authorization: Bearer {}", access_token),
                       "-H", "Content-Type: application/json",
                       "-d", &event_body.to_string(),
                       "https://www.googleapis.com/calendar/v3/calendars/primary/events"])
                .output()
            {
                Ok(output) => {
                    let body = String::from_utf8_lossy(&output.stdout);
                    if body.contains("\"htmlLink\"") {
                        format!("Event created: {}", title)
                    } else {
                        format!("Failed to create event: {}", &body[..body.len().min(200)])
                    }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        _ => format!("Unknown calendar action '{}'. Use: list, create", action),
    }
}

pub fn calendar_auth(args: &HashMap<String, String>) -> String {
    let action = args.get("action").cloned().unwrap_or_else(|| "start".into());

    let config_path = dirs::config_dir()
        .unwrap_or_default()
        .join("nebula/config.json");
    let token_path = dirs::config_dir()
        .unwrap_or_default()
        .join("nebula/google_token.json");

    let config_content = fs::read_to_string(&config_path).unwrap_or_default();
    let config: serde_json::Value = serde_json::from_str(&config_content).unwrap_or_default();

    let client_id = config.get("google")
        .and_then(|g| g.get("client_id"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let client_secret = config.get("google")
        .and_then(|g| g.get("client_secret"))
        .and_then(|c| c.as_str())
        .unwrap_or("");

    if client_id.is_empty() || client_secret.is_empty() {
        return "Error: Set google.client_id and google.client_secret in Settings first.".into();
    }

    match action.as_str() {
        "start" => {
            // Device code flow - step 1
            let result = Command::new("curl")
                .args(["-s", "-X", "POST",
                       "-d", &format!("client_id={}&scope=https://www.googleapis.com/auth/calendar", client_id),
                       "https://oauth2.googleapis.com/device/code"])
                .output();

            match result {
                Ok(output) => {
                    let body = String::from_utf8_lossy(&output.stdout);
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                        let verification_url = json.get("verification_url")
                            .and_then(|u| u.as_str())
                            .unwrap_or("https://www.google.com/device");
                        let user_code = json.get("user_code")
                            .and_then(|c| c.as_str())
                            .unwrap_or("???");
                        let device_code = json.get("device_code")
                            .and_then(|d| d.as_str())
                            .unwrap_or("");

                        // Save device code for polling
                        let pending = serde_json::json!({"device_code": device_code, "client_id": client_id, "client_secret": client_secret});
                        let pending_path = dirs::config_dir().unwrap_or_default().join("nebula/google_pending.json");
                        let _ = fs::write(&pending_path, pending.to_string());

                        format!("Go to {} and enter code: {}\n\nThen tell me to run: [TOOL:calendar_auth action=poll]", verification_url, user_code)
                    } else {
                        format!("Error from Google: {}", &body[..body.len().min(300)])
                    }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "poll" => {
            let pending_path = dirs::config_dir().unwrap_or_default().join("nebula/google_pending.json");
            let pending_content = match fs::read_to_string(&pending_path) {
                Ok(c) => c,
                Err(_) => return "No pending auth. Run calendar_auth action=start first.".into(),
            };
            let pending: serde_json::Value = serde_json::from_str(&pending_content).unwrap_or_default();

            let device_code = pending.get("device_code").and_then(|d| d.as_str()).unwrap_or("");
            let client_id = pending.get("client_id").and_then(|c| c.as_str()).unwrap_or("");
            let client_secret = pending.get("client_secret").and_then(|c| c.as_str()).unwrap_or("");

            let result = Command::new("curl")
                .args(["-s", "-X", "POST",
                       "-d", &format!("client_id={}&client_secret={}&device_code={}&grant_type=urn:ietf:params:oauth:grant-type:device_code",
                                     client_id, client_secret, device_code),
                       "https://oauth2.googleapis.com/token"])
                .output();

            match result {
                Ok(output) => {
                    let body = String::from_utf8_lossy(&output.stdout);
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                        if json.get("access_token").is_some() {
                            let _ = fs::write(&token_path, body.as_ref());
                            let _ = fs::remove_file(&pending_path);
                            "Google Calendar authenticated successfully!".into()
                        } else if let Some(error) = json.get("error").and_then(|e| e.as_str()) {
                            if error == "authorization_pending" {
                                "Still waiting for you to authorize. Visit the URL and enter the code, then try again.".into()
                            } else {
                                format!("Auth error: {}", error)
                            }
                        } else {
                            format!("Unexpected response: {}", &body[..body.len().min(200)])
                        }
                    } else {
                        format!("Parse error: {}", &body[..body.len().min(200)])
                    }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        _ => "Use action=start to begin auth, action=poll after entering the code.".into(),
    }
}

// ─── Network & System Admin Tools ───

pub fn sudo_command(args: &HashMap<String, String>) -> String {
    let command = args.get("command").or_else(|| args.get("cmd")).cloned().unwrap_or_default();
    if command.is_empty() {
        return "Error: no command".into();
    }

    // Run with sudo (requires NOPASSWD in sudoers or cached credentials)
    match Command::new("sudo")
        .args(["-n", "sh", "-c", &command])
        .output()
    {
        Ok(output) => {
            let mut out = String::from_utf8_lossy(&output.stdout).to_string();
            let err = String::from_utf8_lossy(&output.stderr);
            if !err.is_empty() {
                if err.contains("password is required") {
                    return "Error: sudo requires password. Add NOPASSWD for this user or run 'sudo -v' first.".into();
                }
                out.push('\n');
                out.push_str(&err);
            }
            if !output.status.success() {
                out.push_str(&format!("\n(exit {})", output.status.code().unwrap_or(-1)));
            }
            let out = out.trim().to_string();
            if out.is_empty() { "(ok)".into() } else { out[..out.len().min(3000)].to_string() }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn network_scan(args: &HashMap<String, String>) -> String {
    let action = args.get("action").cloned().unwrap_or_else(|| "interfaces".into());
    let target = args.get("target").cloned().unwrap_or_default();

    match action.as_str() {
        "interfaces" => {
            // List network interfaces and IPs
            match Command::new("ip").args(["addr", "show"]).output() {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout);
                    // Parse to something readable
                    let mut results = Vec::new();
                    let mut current_iface = String::new();
                    for line in out.lines() {
                        if !line.starts_with(' ') && line.contains(':') {
                            let parts: Vec<&str> = line.split(':').collect();
                            if parts.len() >= 2 {
                                current_iface = parts[1].trim().to_string();
                            }
                        }
                        if line.contains("inet ") && !line.contains("inet6") {
                            let ip = line.trim().split_whitespace().nth(1).unwrap_or("?");
                            results.push(format!("  {} — {}", current_iface, ip));
                        }
                    }
                    if results.is_empty() { "No interfaces found".into() }
                    else { format!("Network interfaces:\n{}", results.join("\n")) }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "scan" | "discover" => {
            // Scan local network for hosts
            let subnet = if target.is_empty() {
                // Auto-detect subnet
                let output = Command::new("ip").args(["route", "show", "default"]).output();
                match output {
                    Ok(o) => {
                        let out = String::from_utf8_lossy(&o.stdout);
                        // Extract gateway and guess subnet
                        let gw = out.split_whitespace().nth(2).unwrap_or("192.168.1.1");
                        let parts: Vec<&str> = gw.rsplitn(2, '.').collect();
                        format!("{}.0/24", parts.last().unwrap_or(&"192.168.1"))
                    }
                    Err(_) => "192.168.1.0/24".into(),
                }
            } else {
                target.clone()
            };

            // Use nmap if available, fallback to arp-scan or ping sweep
            let result = Command::new("nmap").args(["-sn", "-T4", &subnet]).output();
            match result {
                Ok(output) if output.status.success() => {
                    let out = String::from_utf8_lossy(&output.stdout);
                    let hosts: Vec<&str> = out.lines()
                        .filter(|l| l.contains("Nmap scan report") || l.contains("MAC Address"))
                        .collect();
                    if hosts.is_empty() { format!("No hosts found on {}", subnet) }
                    else { format!("Hosts on {}:\n{}", subnet, hosts.join("\n")[..hosts.join("\n").len().min(2000)].to_string()) }
                }
                _ => {
                    // Fallback: arp-scan
                    match Command::new("sudo").args(["-n", "arp-scan", "--localnet"]).output() {
                        Ok(output) => {
                            let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                            if out.is_empty() { "Install nmap or arp-scan for network scanning".into() }
                            else { out[..out.len().min(2000)].to_string() }
                        }
                        Err(_) => "Install nmap: sudo apt install nmap".into(),
                    }
                }
            }
        }
        "ports" => {
            // Port scan a target
            if target.is_empty() {
                return "Error: need target (IP or hostname)".into();
            }
            let ports = args.get("ports").cloned().unwrap_or_else(|| "1-1024".into());
            match Command::new("nmap").args(["-p", &ports, "-T4", &target]).output() {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if out.is_empty() { "No results (is nmap installed?)".into() }
                    else { out[..out.len().min(2000)].to_string() }
                }
                Err(_) => {
                    // Fallback: use /dev/tcp bash trick for common ports
                    let common_ports = [22, 80, 443, 8080, 3000, 5000, 8000, 3306, 5432, 6379];
                    let mut open = Vec::new();
                    for port in common_ports {
                        let check = Command::new("sh")
                            .arg("-c")
                            .arg(format!("timeout 1 bash -c 'echo >/dev/tcp/{}/{}' 2>/dev/null && echo open", target, port))
                            .output();
                        if let Ok(o) = check {
                            if String::from_utf8_lossy(&o.stdout).contains("open") {
                                open.push(format!("  {} — open", port));
                            }
                        }
                    }
                    if open.is_empty() { format!("No open ports found on {} (common ports only)", target) }
                    else { format!("Open ports on {}:\n{}", target, open.join("\n")) }
                }
            }
        }
        "wifi" => {
            // List available WiFi networks
            match Command::new("nmcli").args(["dev", "wifi", "list"]).output() {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if out.is_empty() { "No WiFi networks found".into() }
                    else { out[..out.len().min(2000)].to_string() }
                }
                Err(_) => "nmcli not available".into(),
            }
        }
        "connections" => {
            // Show active connections
            match Command::new("ss").args(["-tuln"]).output() {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    out[..out.len().min(2000)].to_string()
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "bandwidth" => {
            // Quick bandwidth check using /proc/net/dev
            match Command::new("sh")
                .arg("-c")
                .arg("cat /proc/net/dev | grep -v 'lo:' | tail -n +3 | awk '{print $1, \"RX:\" $2/1024/1024 \"MB TX:\" $10/1024/1024 \"MB\"}'")
                .output()
            {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if out.is_empty() { "No network stats".into() } else { format!("Network usage:\n{}", out) }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        _ => "Actions: interfaces, scan, ports (target=IP), wifi, connections, bandwidth".into(),
    }
}

pub fn bluetooth(args: &HashMap<String, String>) -> String {
    let action = args.get("action").cloned().unwrap_or_else(|| "list".into());
    let device = args.get("device").cloned().unwrap_or_default();

    match action.as_str() {
        "list" | "paired" => {
            match Command::new("bluetoothctl").args(["devices"]).output() {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if out.is_empty() { "No paired Bluetooth devices".into() }
                    else { format!("Paired devices:\n{}", out) }
                }
                Err(_) => "bluetoothctl not available".into(),
            }
        }
        "scan" => {
            // Quick 5-second scan
            match Command::new("sh")
                .arg("-c")
                .arg("timeout 5 bluetoothctl scan on 2>/dev/null & sleep 5; bluetoothctl devices")
                .output()
            {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if out.is_empty() { "No devices found".into() }
                    else { format!("Discovered devices:\n{}", out) }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "connect" => {
            if device.is_empty() { return "Error: need device (MAC address like AA:BB:CC:DD:EE:FF)".into(); }
            match Command::new("bluetoothctl").args(["connect", &device]).output() {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout);
                    let err = String::from_utf8_lossy(&output.stderr);
                    if out.contains("Connected") || out.contains("successful") {
                        format!("Connected to {}", device)
                    } else {
                        format!("Failed: {}{}", out.trim(), err.trim())
                    }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "disconnect" => {
            if device.is_empty() { return "Error: need device MAC".into(); }
            match Command::new("bluetoothctl").args(["disconnect", &device]).output() {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if out.contains("Successful") { format!("Disconnected {}", device) }
                    else { format!("Result: {}", out) }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "info" => {
            if device.is_empty() { return "Error: need device MAC".into(); }
            match Command::new("bluetoothctl").args(["info", &device]).output() {
                Ok(output) => String::from_utf8_lossy(&output.stdout).trim().to_string(),
                Err(e) => format!("Error: {}", e),
            }
        }
        _ => "Actions: list, scan, connect (device=MAC), disconnect (device=MAC), info (device=MAC)".into(),
    }
}

pub fn service_manage(args: &HashMap<String, String>) -> String {
    let action = args.get("action").cloned().unwrap_or_else(|| "status".into());
    let service = args.get("service").cloned().unwrap_or_default();

    match action.as_str() {
        "list" => {
            match Command::new("systemctl").args(["list-units", "--type=service", "--state=running", "--no-pager"]).output() {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    out[..out.len().min(2500)].to_string()
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "status" => {
            if service.is_empty() { return "Error: need service name".into(); }
            match Command::new("systemctl").args(["status", &service, "--no-pager"]).output() {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    out[..out.len().min(1500)].to_string()
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "start" | "stop" | "restart" | "enable" | "disable" => {
            if service.is_empty() { return "Error: need service name".into(); }
            match Command::new("sudo").args(["-n", "systemctl", &action, &service]).output() {
                Ok(output) => {
                    if output.status.success() {
                        format!("Service '{}' — {} done", service, action)
                    } else {
                        let err = String::from_utf8_lossy(&output.stderr);
                        format!("Failed: {}", err.trim())
                    }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "logs" => {
            if service.is_empty() { return "Error: need service name".into(); }
            let lines = args.get("lines").cloned().unwrap_or_else(|| "20".into());
            match Command::new("journalctl").args(["-u", &service, "-n", &lines, "--no-pager"]).output() {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    out[..out.len().min(2500)].to_string()
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        _ => "Actions: list, status (service=X), start/stop/restart/enable/disable (service=X), logs (service=X lines=N)".into(),
    }
}

pub fn security_audit(args: &HashMap<String, String>) -> String {
    let check = args.get("check").cloned().unwrap_or_else(|| "all".into());

    let mut results = Vec::new();

    if check == "all" || check == "ports" {
        // Open ports
        if let Ok(output) = Command::new("ss").args(["-tuln"]).output() {
            let out = String::from_utf8_lossy(&output.stdout);
            let listening: Vec<&str> = out.lines().filter(|l| l.contains("LISTEN")).collect();
            results.push(format!("Open ports ({} listening):\n{}", listening.len(), listening.iter().take(10).cloned().collect::<Vec<_>>().join("\n")));
        }
    }

    if check == "all" || check == "ssh" {
        // SSH config check
        if let Ok(content) = std::fs::read_to_string("/etc/ssh/sshd_config") {
            let mut issues = Vec::new();
            if content.contains("PermitRootLogin yes") { issues.push("  ⚠ Root login enabled"); }
            if content.contains("PasswordAuthentication yes") { issues.push("  ⚠ Password auth enabled (prefer keys)"); }
            if !content.contains("PermitRootLogin no") && !content.contains("PermitRootLogin prohibit-password") {
                issues.push("  ⚠ Root login not explicitly disabled");
            }
            if issues.is_empty() { results.push("SSH: ✓ Looks secure".into()); }
            else { results.push(format!("SSH issues:\n{}", issues.join("\n"))); }
        }
    }

    if check == "all" || check == "firewall" {
        // Firewall status
        if let Ok(output) = Command::new("sudo").args(["-n", "ufw", "status"]).output() {
            let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if out.contains("inactive") { results.push("Firewall: ⚠ UFW inactive".into()); }
            else { results.push(format!("Firewall:\n{}", &out[..out.len().min(500)])); }
        } else if let Ok(output) = Command::new("sudo").args(["-n", "iptables", "-L", "-n"]).output() {
            let out = String::from_utf8_lossy(&output.stdout);
            let rules = out.lines().count();
            results.push(format!("Firewall: iptables ({} rules)", rules));
        }
    }

    if check == "all" || check == "updates" {
        // Pending updates
        if let Ok(output) = Command::new("sh").arg("-c").arg("apt list --upgradable 2>/dev/null | wc -l").output() {
            let count: usize = String::from_utf8_lossy(&output.stdout).trim().parse().unwrap_or(0);
            if count > 1 { results.push(format!("Updates: {} packages upgradable", count - 1)); }
            else { results.push("Updates: ✓ System up to date".into()); }
        }
    }

    if check == "all" || check == "users" {
        // Users with shell access
        if let Ok(content) = std::fs::read_to_string("/etc/passwd") {
            let shell_users: Vec<&str> = content.lines()
                .filter(|l| l.contains("/bin/bash") || l.contains("/bin/zsh") || l.contains("/bin/sh"))
                .filter(|l| !l.starts_with('#'))
                .filter_map(|l| l.split(':').next())
                .collect();
            results.push(format!("Users with shell access: {}", shell_users.join(", ")));
        }
    }

    if check == "all" || check == "permissions" {
        // World-writable files in common locations
        if let Ok(output) = Command::new("find")
            .args(["/home", "-maxdepth", "3", "-perm", "-o+w", "-type", "f"])
            .output()
        {
            let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let count = out.lines().count();
            if count > 0 {
                results.push(format!("World-writable files: {} found in /home", count));
            } else {
                results.push("Permissions: ✓ No world-writable files in /home".into());
            }
        }
    }

    if results.is_empty() { "No security checks completed.".into() }
    else { format!("Security Audit:\n\n{}", results.join("\n\n")) }
}

// ─── Agentic Tools (v4) ───

pub fn configure_workspace(args: &HashMap<String, String>) -> String {
    let mode = args.get("mode").cloned().unwrap_or_else(|| "coding".into());
    let sidecar = dirs::home_dir().unwrap_or_default().join("jarvis-v3/sidecar/workspace.py");

    match Command::new("python3").arg(&sidecar).arg(&mode).output() {
        Ok(output) => {
            let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if out.is_empty() { format!("Workspace set to: {}", mode) } else { out }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn vision_snapshot(args: &HashMap<String, String>) -> String {
    let prompt = args.get("prompt").cloned().unwrap_or_else(|| "Describe what you see on the desk. List all objects.".into());
    let sidecar = dirs::home_dir().unwrap_or_default().join("jarvis-v3/sidecar/vision.py");

    match Command::new("python3").arg(&sidecar).output() {
        Ok(output) => {
            let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if out.is_empty() {
                "Could not capture vision. Ensure webcam is available.".into()
            } else {
                out
            }
        }
        Err(e) => format!("Vision error: {}", e),
    }
}

pub fn screenshot(args: &HashMap<String, String>) -> String {
    let output_path = "/tmp/nebula_screenshot.png";

    // Try grim (Wayland), then scrot (X11), then import (ImageMagick)
    let result = Command::new("grim").arg(output_path).output()
        .or_else(|_| Command::new("scrot").arg(output_path).output())
        .or_else(|_| Command::new("import").args(["-window", "root", output_path]).output());

    match result {
        Ok(output) if output.status.success() => {
            format!("Screenshot saved to {}", output_path)
        }
        _ => "Failed to capture screenshot. No screenshot tool available.".into(),
    }
}

pub fn analyze_screen(args: &HashMap<String, String>) -> String {
    let prompt = args.get("prompt").cloned().unwrap_or_else(|| "Describe what's on this screen. What applications are open? What is the user working on?".into());
    let screenshot_path = "/tmp/nebula_screenshot.png";

    // Capture screenshot first
    let cap_result = Command::new("grim").arg(screenshot_path).output()
        .or_else(|_| Command::new("scrot").arg(screenshot_path).output())
        .or_else(|_| Command::new("import").args(["-window", "root", screenshot_path]).output());

    match cap_result {
        Ok(output) if output.status.success() => {}
        _ => return "Failed to capture screen.".into(),
    }

    // Send to Claude Vision via Python sidecar
    let sidecar = dirs::home_dir().unwrap_or_default().join("jarvis-v3/sidecar/analyze_image.py");
    match Command::new("python3")
        .arg(&sidecar)
        .arg(screenshot_path)
        .arg(&prompt)
        .output()
    {
        Ok(output) => {
            let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if out.is_empty() {
                "Could not analyze screenshot.".into()
            } else {
                out
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

// ─── Productivity Tools ───

pub fn timer(args: &HashMap<String, String>) -> String {
    let action = args.get("action").cloned().unwrap_or_else(|| "start".into());
    let minutes = args.get("minutes").or(args.get("duration")).and_then(|m| m.parse::<u32>().ok()).unwrap_or(25);
    let label = args.get("label").cloned().unwrap_or_else(|| "Timer".into());

    match action.as_str() {
        "start" => {
            // Run timer in background, notify when done
            let cmd = format!(
                "nohup sh -c 'sleep {} && notify-send \"JARVIS Timer\" \"{}\" && paplay /usr/share/sounds/freedesktop/stereo/complete.oga 2>/dev/null' >/dev/null 2>&1 &",
                minutes * 60, format!("{} — {} minutes complete", label, minutes)
            );
            let _ = Command::new("sh").arg("-c").arg(&cmd).spawn();
            format!("Timer set: {} minutes ({}). I will notify you when it is done.", minutes, label)
        }
        "pomodoro" => {
            let cmd = "nohup sh -c 'sleep 1500 && notify-send \"JARVIS\" \"Pomodoro complete. Take a 5 minute break.\" && paplay /usr/share/sounds/freedesktop/stereo/complete.oga 2>/dev/null' >/dev/null 2>&1 &";
            let _ = Command::new("sh").arg("-c").arg(cmd).spawn();
            "Pomodoro started: 25 minutes of focus. I will alert you when it is time for a break.".into()
        }
        "cancel" => {
            let _ = Command::new("pkill").args(["-f", "sleep.*notify-send.*Timer"]).output();
            "All timers cancelled.".into()
        }
        _ => "Actions: start (minutes=N label=X), pomodoro, cancel".into(),
    }
}

pub fn weather(args: &HashMap<String, String>) -> String {
    let location = args.get("location").cloned().unwrap_or_default();

    let url = if location.is_empty() {
        "https://wttr.in/?format=%C+%t+%h+%w".to_string()
    } else {
        format!("https://wttr.in/{}?format=%C+%t+%h+%w", urlencoding::encode(&location))
    };

    match Command::new("curl").args(["-s", "-m", "5", &url]).output() {
        Ok(output) => {
            let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if out.is_empty() || out.contains("Unknown") {
                "Could not fetch weather.".into()
            } else {
                format!("Weather: {}", out)
            }
        }
        Err(e) => format!("Error: {}", e),
    }
}

pub fn todo(args: &HashMap<String, String>) -> String {
    let action = args.get("action").cloned().unwrap_or_else(|| "list".into());
    let item = args.get("item").or(args.get("task")).cloned().unwrap_or_default();

    let todo_path = dirs::home_dir()
        .unwrap_or_default()
        .join("Documents/vault/JARVIS/memory/todos.md");

    match action.as_str() {
        "add" => {
            if item.is_empty() { return "Error: need item".into(); }
            let existing = fs::read_to_string(&todo_path).unwrap_or_default();
            let new_content = format!("{}- [ ] {}\n", existing, item);
            let _ = fs::write(&todo_path, new_content);
            format!("Added: {}", item)
        }
        "done" | "complete" => {
            if item.is_empty() { return "Error: need item text to mark done".into(); }
            let content = fs::read_to_string(&todo_path).unwrap_or_default();
            let new_content = content.replace(
                &format!("- [ ] {}", item),
                &format!("- [x] {}", item)
            );
            let _ = fs::write(&todo_path, new_content);
            format!("Completed: {}", item)
        }
        "remove" | "delete" => {
            if item.is_empty() { return "Error: need item".into(); }
            let content = fs::read_to_string(&todo_path).unwrap_or_default();
            let new_content: String = content.lines()
                .filter(|l| !l.contains(&item))
                .collect::<Vec<_>>()
                .join("\n") + "\n";
            let _ = fs::write(&todo_path, new_content);
            format!("Removed: {}", item)
        }
        "clear" => {
            let content = fs::read_to_string(&todo_path).unwrap_or_default();
            let new_content: String = content.lines()
                .filter(|l| !l.contains("[x]"))
                .collect::<Vec<_>>()
                .join("\n") + "\n";
            let _ = fs::write(&todo_path, new_content);
            "Cleared completed items.".into()
        }
        _ => {
            // list
            let content = fs::read_to_string(&todo_path).unwrap_or_default();
            let todos: Vec<&str> = content.lines().filter(|l| l.contains("- [")).collect();
            if todos.is_empty() {
                "No todos. Add with: todo action=add item=...".into()
            } else {
                format!("Todos:\n{}", todos.join("\n"))
            }
        }
    }
}

pub fn spotify(args: &HashMap<String, String>) -> String {
    let action = args.get("action").cloned().unwrap_or_else(|| "status".into());

    match action.as_str() {
        "play" | "resume" => {
            let _ = Command::new("playerctl").args(["play"]).output();
            "Playing.".into()
        }
        "pause" | "stop" => {
            let _ = Command::new("playerctl").args(["pause"]).output();
            "Paused.".into()
        }
        "next" | "skip" => {
            let _ = Command::new("playerctl").args(["next"]).output();
            "Skipped.".into()
        }
        "prev" | "previous" | "back" => {
            let _ = Command::new("playerctl").args(["previous"]).output();
            "Previous track.".into()
        }
        "volume" => {
            let level = args.get("level").cloned().unwrap_or_else(|| "50".into());
            let vol = format!("{}%", level.trim_end_matches('%'));
            let _ = Command::new("playerctl").args(["volume", &format!("0.{}", level.trim_end_matches('%'))]).output();
            format!("Volume set to {}.", vol)
        }
        _ => {
            // status / now playing
            match Command::new("playerctl").args(["metadata", "--format", "{{artist}} - {{title}} ({{status}})"]).output() {
                Ok(output) => {
                    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if out.is_empty() || out.contains("No players found") {
                        "No music playing.".into()
                    } else {
                        format!("Now playing: {}", out)
                    }
                }
                Err(_) => "playerctl not available. Install: sudo apt install playerctl".into(),
            }
        }
    }
}

pub fn file_manager(args: &HashMap<String, String>) -> String {
    let action = args.get("action").cloned().unwrap_or_else(|| "list".into());
    let path = args.get("path").cloned().unwrap_or_else(|| "~".into());
    let expanded = shellexpand::tilde(&path);

    match action.as_str() {
        "open" => {
            let _ = Command::new("xdg-open").arg(expanded.as_ref()).spawn();
            format!("Opened: {}", path)
        }
        "size" => {
            match Command::new("du").args(["-sh", expanded.as_ref()]).output() {
                Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "recent" => {
            match Command::new("sh").arg("-c")
                .arg(format!("find {} -maxdepth 2 -type f -mmin -60 -printf '%T+ %p\\n' 2>/dev/null | sort -r | head -10", expanded))
                .output()
            {
                Ok(o) => {
                    let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if out.is_empty() { "No recently modified files.".into() } else { format!("Recent files:\n{}", out) }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "trash" => {
            let target = args.get("file").cloned().unwrap_or_default();
            if target.is_empty() { return "Error: need file".into(); }
            let expanded_target = shellexpand::tilde(&target);
            match Command::new("gio").args(["trash", expanded_target.as_ref()]).output() {
                Ok(o) if o.status.success() => format!("Trashed: {}", target),
                _ => {
                    // Fallback: move to ~/.local/share/Trash
                    let trash_dir = dirs::home_dir().unwrap_or_default().join(".local/share/Trash/files");
                    let _ = fs::create_dir_all(&trash_dir);
                    let filename = std::path::Path::new(expanded_target.as_ref()).file_name().unwrap_or_default().to_string_lossy().to_string();
                    match fs::rename(expanded_target.as_ref(), trash_dir.join(&filename)) {
                        Ok(_) => format!("Trashed: {}", target),
                        Err(e) => format!("Error: {}", e),
                    }
                }
            }
        }
        _ => {
            // list
            list_files(args)
        }
    }
}

// ─── Tool Registry ───

pub struct ToolRegistry {
    custom_tools_dir: PathBuf,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let custom_tools_dir = dirs::home_dir()
            .unwrap_or_default()
            .join("Documents/vault/JARVIS/tools");
        let _ = fs::create_dir_all(&custom_tools_dir);

        Self { custom_tools_dir }
    }

    pub fn execute(&self, name: &str, args: &HashMap<String, String>) -> String {
        match name {
            "run_command" => run_command(args),
            "read_file" => read_file(args),
            "write_file" => write_file(args),
            "list_files" => list_files(args),
            "web_search" => web_search(args),
            "web_fetch" => web_fetch(args),
            "clipboard" => clipboard(args),
            "notify" => notify(args),
            "open_url" => open_url(args),
            "datetime" => get_datetime(args),
            "calc" => calc(args),
            "system_info" => system_info(args),
            "remember" => remember(args),
            "knowledge_search" => knowledge_search(args),
            "knowledge_save" => knowledge_save(args),
            "save_decision" => save_decision(args),
            "save_idea" => save_idea(args),
            "save_learning" => save_learning(args),
            "save_person" => save_person(args),
            "recall" => recall(args),
            "daily_summary" => daily_summary(args),
            "pip_install" => pip_install(args),
            "git" => git(args),
            "multi_command" => multi_command(args),
            "find_files" => find_files(args),
            "replace_in_file" => replace_in_file(args),
            "processes" => processes(args),
            "kill_process" => kill_process(args),
            "set_context" => set_context(args),
            "google_search" => google_search(args),
            "calendar" => calendar(args),
            "calendar_auth" => calendar_auth(args),
            "sudo" => sudo_command(args),
            "network" => network_scan(args),
            "bluetooth" => bluetooth(args),
            "service" => service_manage(args),
            "security_audit" => security_audit(args),
            "configure_workspace" => configure_workspace(args),
            "vision_snapshot" => vision_snapshot(args),
            "screenshot" => screenshot(args),
            "analyze_screen" => analyze_screen(args),
            "timer" => timer(args),
            "weather" => weather(args),
            "todo" => todo(args),
            "spotify" => spotify(args),
            "file_manager" => file_manager(args),
            _ => self.run_custom_tool(name, args),
        }
    }

    fn run_custom_tool(&self, name: &str, args: &HashMap<String, String>) -> String {
        let script_path = self.custom_tools_dir.join(format!("{}.ts", name));
        if !script_path.exists() {
            return format!(
                "Unknown tool '{}'. Available: run_command, sudo, read_file, write_file, list_files, \
                 web_search, google_search, web_fetch, clipboard, notify, open_url, datetime, calc, \
                 system_info, remember, knowledge_search, knowledge_save, save_decision, save_idea, \
                 save_learning, save_person, recall, daily_summary, pip_install, git, \
                 multi_command, find_files, replace_in_file, processes, kill_process, set_context, \
                 calendar, calendar_auth, network, bluetooth, service, security_audit",
                name
            );
        }

        let args_json = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());

        match Command::new("bun")
            .arg("run")
            .arg(&script_path)
            .env("TOOL_ARGS", &args_json)
            .output()
        {
            Ok(output) => {
                let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if !err.is_empty() && out.is_empty() {
                    format!("Error: {}", err)
                } else if out.is_empty() {
                    "(no output)".into()
                } else {
                    out[..out.len().min(3000)].to_string()
                }
            }
            Err(e) => format!("Error running custom tool: {}", e),
        }
    }

    pub fn create_tool(&self, name: &str, code: &str) -> String {
        let script_path = self.custom_tools_dir.join(format!("{}.ts", name));

        // Clean up code
        let mut clean = code.trim().to_string();
        if clean.starts_with("```typescript") || clean.starts_with("```ts") {
            clean = clean.splitn(2, '\n').nth(1).unwrap_or("").to_string();
        }
        if clean.starts_with("```") {
            clean = clean[3..].to_string();
        }
        if clean.ends_with("```") {
            clean = clean[..clean.len() - 3].to_string();
        }
        let clean = clean.trim();

        // Ensure it reads TOOL_ARGS
        let final_code = if clean.contains("TOOL_ARGS") {
            clean.to_string()
        } else {
            format!(
                "const args = JSON.parse(process.env.TOOL_ARGS || \"{{}}\");\n\n{}\n",
                clean
            )
        };

        match fs::write(&script_path, &final_code) {
            Ok(_) => format!("Created tool '{}' at {}", name, script_path.display()),
            Err(e) => format!("Error: {}", e),
        }
    }

    pub fn list_names(&self) -> Vec<String> {
        let mut names = vec![
            "run_command", "read_file", "write_file", "list_files",
            "web_search", "web_fetch", "clipboard", "notify", "open_url",
            "datetime", "calc", "system_info", "remember", "knowledge_search",
            "knowledge_save", "save_decision", "save_idea", "save_learning",
            "save_person", "recall", "daily_summary", "pip_install", "git",
            "multi_command", "find_files", "replace_in_file", "processes",
            "kill_process", "set_context",
        ].into_iter().map(String::from).collect::<Vec<_>>();

        // Add custom tools
        if let Ok(entries) = fs::read_dir(&self.custom_tools_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".ts") {
                    names.push(name.trim_end_matches(".ts").to_string());
                }
            }
        }

        names.sort();
        names
    }
}
