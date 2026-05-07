use regex::Regex;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

pub struct VoiceEngine {
    voice: String,
    rate: String,
    pitch: String,
    pub enabled: AtomicBool,
    tx: mpsc::Sender<String>,
}

impl VoiceEngine {
    pub fn new(_model_path: String, speed: f32, enabled: bool) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<String>();

        // Convert speed to edge-tts rate format (e.g. 1.25 -> "+25%")
        let rate_pct = ((speed - 1.0) * 100.0).round() as i32;
        let rate = if rate_pct >= 0 {
            format!("+{}%", rate_pct)
        } else {
            format!("{}%", rate_pct)
        };

        // Pitch: negative = deeper voice
        let pitch = "-15Hz".to_string();

        let engine = Arc::new(Self {
            voice: "en-GB-RyanNeural".into(),
            rate,
            pitch,
            enabled: AtomicBool::new(enabled),
            tx,
        });

        let voice = engine.voice.clone();
        let rate_clone = engine.rate.clone();
        let pitch_clone = engine.pitch.clone();
        let engine_clone = engine.clone();

        thread::spawn(move || {
            speech_worker_loop(rx, &voice, &rate_clone, &pitch_clone, engine_clone);
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
    voice: &str,
    rate: &str,
    pitch: &str,
    engine: Arc<VoiceEngine>,
) {
    // Use a consistent temp file path
    let tmp_path = "/tmp/jarvis-tts.mp3";

    while let Ok(text) = rx.recv() {
        if !engine.enabled.load(Ordering::Relaxed) {
            continue;
        }

        // Generate audio with edge-tts
        let tts_result = Command::new("edge-tts")
            .args([
                "-t", &text,
                "-v", voice,
                "--rate", rate,
                "--pitch", pitch,
                "--write-media", tmp_path,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        match tts_result {
            Ok(status) if status.success() => {}
            _ => continue,
        }

        // Play with ffplay (no window, quiet)
        let _ = Command::new("ffplay")
            .args([
                "-nodisp", "-autoexit", "-loglevel", "quiet",
                tmp_path,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
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
