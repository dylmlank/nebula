use regex::Regex;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

const PIPER_BIN: &str = "/home/dylan/.local/bin/piper/piper";
const DEFAULT_MODEL: &str = "/home/dylan/.local/share/piper-voices/en_US-ryan-high.onnx";

pub struct VoiceEngine {
    model_path: String,
    length_scale: String,
    pub enabled: AtomicBool,
    tx: mpsc::Sender<String>,
}

impl VoiceEngine {
    pub fn new(model_path: String, speed: f32, enabled: bool) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<String>();

        // Use provided model or default to high-quality ryan
        let model = if model_path.is_empty() || model_path.contains("edge") || model_path.contains("Neural") {
            DEFAULT_MODEL.to_string()
        } else {
            model_path
        };

        // length_scale: higher = slower/deeper. Config speed IS the length_scale.
        let length_scale = format!("{:.2}", speed);

        let engine = Arc::new(Self {
            model_path: model.clone(),
            length_scale: length_scale.clone(),
            enabled: AtomicBool::new(enabled),
            tx,
        });

        let engine_clone = engine.clone();

        thread::spawn(move || {
            speech_worker_loop(rx, &model, &length_scale, engine_clone);
        });

        engine
    }

    /// Queue text for speaking (non-blocking)
    pub fn speak(&self, text: &str) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        let clean = clean_for_speech(text);
        if clean.trim().is_empty() || clean.len() < 3 {
            return;
        }

        let _ = self.tx.send(clean);
    }

    pub fn toggle(&self) -> bool {
        let new_val = !self.enabled.load(Ordering::Relaxed);
        self.enabled.store(new_val, Ordering::Relaxed);
        new_val
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
}

/// Background worker that processes speech queue sequentially
fn speech_worker_loop(
    rx: mpsc::Receiver<String>,
    model_path: &str,
    length_scale: &str,
    engine: Arc<VoiceEngine>,
) {
    while let Ok(text) = rx.recv() {
        if !engine.enabled.load(Ordering::Relaxed) {
            continue;
        }

        // Spawn piper | aplay pipeline — zero latency, streams directly
        let piper = Command::new(PIPER_BIN)
            .args([
                "--model", model_path,
                "--output_raw",
                "--length-scale", length_scale,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();

        let mut piper = match piper {
            Ok(p) => p,
            Err(_) => continue,
        };

        let piper_stdout = match piper.stdout.take() {
            Some(s) => s,
            None => continue,
        };

        let mut aplay = match Command::new("aplay")
            .args(["-r", "22050", "-f", "S16_LE", "-t", "raw", "-"])
            .stdin(piper_stdout)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(a) => a,
            Err(_) => continue,
        };

        // Write text to piper stdin
        if let Some(mut stdin) = piper.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
            // Drop stdin to close it, signaling EOF to piper
        }

        // Wait for aplay to finish
        let _ = aplay.wait();
        let _ = piper.wait();
    }
}

/// Clean text for speech — remove tool tokens, code, and noise
fn clean_for_speech(text: &str) -> String {
    let tool_re = Regex::new(r"\[TOOL:\w+.*?\]").unwrap();
    let create_re = Regex::new(r"(?s)\[CREATE_TOOL:\w+\].*?\[/CREATE_TOOL\]").unwrap();
    let code_re = Regex::new(r"(?s)```.*?```").unwrap();

    let text = tool_re.replace_all(text, "");
    let text = create_re.replace_all(&text, "");
    let text = code_re.replace_all(&text, "");

    // Remove remaining symbols
    let result: String = text
        .chars()
        .filter(|c| !['[', ']', '`', '#', '*', '_', '{', '}'].contains(c))
        .collect();

    // Collapse whitespace
    let multi_newline = Regex::new(r"\n{3,}").unwrap();
    let multi_space = Regex::new(r"  +").unwrap();
    let result = multi_newline.replace_all(&result, "\n\n").to_string();
    let result = multi_space.replace_all(&result, " ").to_string();

    result.trim().to_string()
}
