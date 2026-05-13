use regex::Regex;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

const PIPER_BIN: &str = "/home/dylan/.local/bin/piper/piper";
const DEFAULT_MODEL: &str = "/home/dylan/.local/share/piper-voices/en_US-ryan-high.onnx";
const SAMPLE_RATE: &str = "22050";

pub struct VoiceEngine {
    _model_path: String,
    _length_scale: String,
    pub enabled: AtomicBool,
    tx: mpsc::Sender<VoiceCmd>,
}

enum VoiceCmd {
    Speak(String),
    Stop,
}

impl VoiceEngine {
    pub fn new(model_path: String, speed: f32, enabled: bool) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<VoiceCmd>();

        let model = if model_path.is_empty() || model_path.contains("edge") || model_path.contains("Neural") {
            DEFAULT_MODEL.to_string()
        } else {
            shellexpand::tilde(&model_path).to_string()
        };

        let length_scale = format!("{:.2}", speed);

        let engine = Arc::new(Self {
            _model_path: model.clone(),
            _length_scale: length_scale.clone(),
            enabled: AtomicBool::new(enabled),
            tx,
        });

        let engine_clone = engine.clone();
        thread::spawn(move || {
            speech_worker(rx, &model, &length_scale, engine_clone);
        });

        engine
    }

    /// Queue text for speaking — splits into sentences for faster first-audio
    pub fn speak(&self, text: &str) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        let clean = clean_for_speech(text);
        if clean.trim().is_empty() || clean.len() < 3 {
            return;
        }

        // Split into sentences — speak each independently for lower latency
        // First sentence starts playing while piper processes the rest
        let sentences = split_sentences(&clean);
        for s in sentences {
            if s.len() >= 3 {
                let _ = self.tx.send(VoiceCmd::Speak(s));
            }
        }
    }

    /// Stop all queued and current speech
    pub fn stop(&self) {
        let _ = self.tx.send(VoiceCmd::Stop);
        // Kill any running aplay immediately
        let _ = Command::new("pkill").args(["-9", "-f", "aplay.*S16_LE"]).output();
        let _ = Command::new("pkill").args(["-9", "-f", "piper"]).output();
    }

    pub fn toggle(&self) -> bool {
        let new_val = !self.enabled.load(Ordering::Relaxed);
        self.enabled.store(new_val, Ordering::Relaxed);
        if !new_val {
            self.stop();
        }
        new_val
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
}

/// Background worker — processes speech queue sentence by sentence
fn speech_worker(
    rx: mpsc::Receiver<VoiceCmd>,
    model_path: &str,
    length_scale: &str,
    engine: Arc<VoiceEngine>,
) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            VoiceCmd::Stop => {
                // Drain remaining queued items
                while rx.try_recv().is_ok() {}
                continue;
            }
            VoiceCmd::Speak(text) => {
                if !engine.enabled.load(Ordering::Relaxed) {
                    continue;
                }
                speak_one(&text, model_path, length_scale);
            }
        }
    }
}

/// Speak a single sentence via piper|aplay
fn speak_one(text: &str, model_path: &str, length_scale: &str) {
    let piper = Command::new(PIPER_BIN)
        .args(["--model", model_path, "--output_raw", "--length-scale", length_scale])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();

    let mut piper = match piper {
        Ok(p) => p,
        Err(_) => return,
    };

    let piper_stdout = match piper.stdout.take() {
        Some(s) => s,
        None => return,
    };

    let mut aplay = match Command::new("aplay")
        .args(["-r", SAMPLE_RATE, "-f", "S16_LE", "-t", "raw", "-"])
        .stdin(piper_stdout)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(a) => a,
        Err(_) => return,
    };

    if let Some(mut stdin) = piper.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }

    let _ = aplay.wait();
    let _ = piper.wait();
}

/// Split text into sentences for streaming TTS
fn split_sentences(text: &str) -> Vec<String> {
    // Split on sentence-ending punctuation followed by space
    let parts: Vec<&str> = text.split_inclusive(|c: char| c == '.' || c == '!' || c == '?')
        .collect();

    if parts.len() <= 1 {
        return vec![text.to_string()];
    }

    parts.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Clean text for speech — strip tool tokens, code, markdown, URLs
fn clean_for_speech(text: &str) -> String {
    // These regexes are compiled fresh each call but it's fast enough (~1μs)
    // since this only runs once per response, not per token
    let tool_re = Regex::new(r"\[TOOL:\w+.*?\]").unwrap();
    let create_re = Regex::new(r"(?s)\[CREATE_TOOL:\w+\].*?\[/CREATE_TOOL\]").unwrap();
    let code_re = Regex::new(r"(?s)```.*?```").unwrap();
    let url_re = Regex::new(r"https?://\S+").unwrap();

    let text = tool_re.replace_all(text, "");
    let text = create_re.replace_all(&text, "");
    let text = code_re.replace_all(&text, " ");
    let text = url_re.replace_all(&text, "");

    // Remove markdown symbols and file paths
    let result: String = text
        .chars()
        .filter(|c| !['[', ']', '`', '#', '*', '_', '{', '}', '~'].contains(c))
        .collect();

    // Remove file paths
    let path_re = Regex::new(r"/[\w/.\-]+").unwrap();
    let result = path_re.replace_all(&result, "");

    // Collapse whitespace
    let multi_nl = Regex::new(r"\n{3,}").unwrap();
    let multi_sp = Regex::new(r"  +").unwrap();
    let result = multi_nl.replace_all(&result, "\n\n").to_string();
    let result = multi_sp.replace_all(&result, " ").to_string();

    result.trim().to_string()
}
