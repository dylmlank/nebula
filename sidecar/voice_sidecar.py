#!/usr/bin/env python3
"""JARVIS Voice Sidecar — Vosk STT + noise reduction.

Uses Vosk for fast streaming speech recognition (much better for conversation)
and noisereduce for cleaning up audio before recognition.

Protocol (stdout JSON):
  {"type": "ready"}
  {"type": "wake"}        — ready to listen
  {"type": "listening"}   — recording speech
  {"type": "transcript", "text": "..."} — speech transcribed
  {"type": "interrupt"}   — user interrupted JARVIS
  {"type": "resume"}      — user said resume/continue
  {"type": "status", "message": "..."} — loading status
  {"type": "error", "message": "..."} — error

Protocol (stdin):
  "quit" — shutdown
  "speaking" — JARVIS is talking (pause listening)
  "done_speaking" — JARVIS stopped (resume listening)
"""

import json
import sys
import os
import time
import threading
import numpy as np

def emit(event_type: str, **kwargs):
    data = {"type": event_type, **kwargs}
    print(json.dumps(data), flush=True)


def main():
    emit("status", message="Starting voice engine...")

    try:
        import sounddevice as sd
    except ImportError:
        emit("error", message="Install sounddevice: pip install sounddevice")
        return

    try:
        from faster_whisper import WhisperModel
    except ImportError:
        emit("error", message="Install faster-whisper: pip install faster-whisper")
        return

    try:
        import noisereduce as nr
    except ImportError:
        nr = None

    # Load Whisper small model (accurate transcription)
    emit("status", message="Loading speech model...")
    model = WhisperModel("small", device="cpu", compute_type="int8")

    emit("ready")

    # Config
    SAMPLE_RATE = 16000
    CHANNELS = 1
    SPEECH_THRESHOLD = 0.02  # RMS to start recording (lowered for quiet mics)
    SILENCE_THRESHOLD = 0.008  # RMS for silence detection
    SILENCE_DURATION = 0.6  # seconds of silence to end recording (snappy cutoff)

    should_quit = False
    jarvis_speaking = [False]
    SPEAKING_FLAG = "/tmp/jarvis_speaking"

    def stdin_reader():
        nonlocal should_quit
        for line in sys.stdin:
            cmd = line.strip().lower()
            if cmd == "quit":
                should_quit = True
                break
            elif cmd == "speaking":
                jarvis_speaking[0] = True
            elif cmd == "done_speaking":
                jarvis_speaking[0] = False

    def is_jarvis_speaking():
        """Check both stdin flag and file flag."""
        if jarvis_speaking[0]:
            return True
        # File-based signal (set by Rust voice engine)
        return os.path.exists(SPEAKING_FLAG)

    stdin_thread = threading.Thread(target=stdin_reader, daemon=True)
    stdin_thread.start()

    # Ensure mic is unmuted on start (safety — in case stale mute from crash)
    import subprocess
    subprocess.run(["pactl", "set-source-mute", "@DEFAULT_SOURCE@", "0"], capture_output=True)
    # Clear stale speaking flag
    if os.path.exists("/tmp/jarvis_speaking"):
        os.remove("/tmp/jarvis_speaking")

    emit("wake")

    while not should_quit:
        # Skip while JARVIS is speaking
        if is_jarvis_speaking():
            time.sleep(0.1)
            continue

        # Poll for speech
        chunk = sd.rec(int(SAMPLE_RATE * 0.3), samplerate=SAMPLE_RATE, channels=CHANNELS, dtype='float32')
        sd.wait()

        rms = np.sqrt(np.mean(chunk ** 2))

        if rms < SPEECH_THRESHOLD:
            continue

        # Speech detected — record full utterance
        emit("listening")
        audio_parts = [chunk]
        silence_counter = 0

        while silence_counter < SILENCE_DURATION:
            if is_jarvis_speaking():
                break

            part = sd.rec(int(SAMPLE_RATE * 0.2), samplerate=SAMPLE_RATE, channels=CHANNELS, dtype='float32')
            sd.wait()
            audio_parts.append(part)

            part_rms = np.sqrt(np.mean(part ** 2))
            if part_rms < SILENCE_THRESHOLD:
                silence_counter += 0.2
            else:
                silence_counter = 0

            # Max 30s
            if sum(len(p) for p in audio_parts) / SAMPLE_RATE > 30:
                break

        if is_jarvis_speaking():
            emit("wake")
            continue

        # Combine audio
        audio = np.concatenate(audio_parts).flatten()

        # Apply noise reduction if available
        if nr is not None:
            try:
                audio = nr.reduce_noise(y=audio, sr=SAMPLE_RATE, stationary=True, prop_decrease=0.8)
            except:
                pass

        # Transcribe with Whisper small (accurate)
        segments, _ = model.transcribe(audio, language="en", beam_size=3, vad_filter=True)
        text = " ".join(s.text.strip() for s in segments).strip()

        if text and len(text) > 1:
            text_lower = text.lower()
            if text_lower in ("resume", "continue", "go on", "keep going"):
                emit("resume")
            else:
                emit("interrupt")
                time.sleep(0.05)
                emit("transcript", text=text)

        emit("wake")


if __name__ == "__main__":
    main()
