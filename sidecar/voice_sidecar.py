#!/usr/bin/env python3
"""JARVIS Voice Sidecar — Wake word detection + Whisper STT.

Runs continuously, listens for "jarvis" wake word, then transcribes speech.
Outputs JSON lines to stdout for the Tauri app to consume.

Protocol (stdout JSON lines):
  {"type": "ready"}                     — sidecar initialized
  {"type": "wake"}                      — wake word detected, now listening
  {"type": "listening"}                 — actively recording user speech
  {"type": "transcript", "text": "..."}  — speech transcribed, ready to process
  {"type": "error", "message": "..."}   — error occurred

Protocol (stdin lines):
  "stop"     — stop current recording
  "quit"     — shutdown sidecar
"""

import json
import sys
import os
import time
import threading
import numpy as np
import queue

# Suppress warnings
os.environ["KMP_DUPLICATE_LIB_OK"] = "TRUE"

def emit(event_type: str, **kwargs):
    """Send event to Tauri app."""
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
        emit("status", message="Loading speech model (first time takes ~5s)...")
        from faster_whisper import WhisperModel
    except ImportError:
        emit("error", message="Install faster-whisper: pip install faster-whisper")
        return

    # Load Whisper model (tiny for speed — wake word doesn't need accuracy)
    model = WhisperModel("tiny", device="cpu", compute_type="int8")
    emit("ready")

    # Audio config
    SAMPLE_RATE = 16000
    CHANNELS = 1
    BLOCK_SIZE = 1024
    SPEECH_THRESHOLD = 0.04  # RMS level that indicates actual speech (not just noise)
    SILENCE_THRESHOLD = 0.015  # Below this = silence (for end-of-speech detection)
    SILENCE_DURATION = 1.5  # seconds of silence to stop recording
    WAKE_WORDS = ["jarvis", "hey jarvis", "yo jarvis", "okay jarvis"]

    audio_queue = queue.Queue()
    is_listening = False
    wake_detected = False
    should_quit = False

    def audio_callback(indata, frames, time_info, status):
        """Called for each audio block from the microphone."""
        audio_queue.put(indata.copy())

    def detect_wake_word(audio_data: np.ndarray) -> bool:
        """Check if audio contains the wake word using Whisper."""
        # Only process if there's speech-level audio (not just ambient noise)
        rms = np.sqrt(np.mean(audio_data ** 2))
        if rms < SPEECH_THRESHOLD:
            return False

        # Transcribe with tiny model
        segments, _ = model.transcribe(
            audio_data.flatten().astype(np.float32),
            language="en",
            vad_filter=True,
            beam_size=1,
        )

        text = " ".join(s.text.lower().strip() for s in segments)
        if text:
            emit("status", message=f"Heard: {text[:30]}")
        return any(wake in text for wake in WAKE_WORDS)

    def transcribe_speech(audio_data: np.ndarray) -> str:
        """Transcribe full speech with better accuracy."""
        segments, _ = model.transcribe(
            audio_data.flatten().astype(np.float32),
            language="en",
            vad_filter=True,
            beam_size=3,
        )
        return " ".join(s.text.strip() for s in segments).strip()

    def stdin_reader():
        """Read commands from Tauri app."""
        nonlocal should_quit
        for line in sys.stdin:
            cmd = line.strip().lower()
            if cmd == "quit":
                should_quit = True
                break

    # Start stdin reader thread
    stdin_thread = threading.Thread(target=stdin_reader, daemon=True)
    stdin_thread.start()

    # Main loop
    try:
        with sd.InputStream(samplerate=SAMPLE_RATE, channels=CHANNELS,
                           blocksize=BLOCK_SIZE, callback=audio_callback):
            speech_buffer = []
            silence_counter = 0
            is_speaking = False

            emit("wake")  # Immediately ready to listen

            while not should_quit:
                try:
                    data = audio_queue.get(timeout=0.1)
                except queue.Empty:
                    continue

                rms = np.sqrt(np.mean(data ** 2))

                if not is_speaking:
                    # Waiting for user to start talking
                    if rms >= SPEECH_THRESHOLD:
                        is_speaking = True
                        speech_buffer = [data]
                        silence_counter = 0
                        emit("listening")
                else:
                    # Recording user speech
                    speech_buffer.append(data)

                    if rms < SILENCE_THRESHOLD:
                        silence_counter += len(data) / SAMPLE_RATE
                    else:
                        silence_counter = 0

                    # Stop after silence
                    if silence_counter >= SILENCE_DURATION and len(speech_buffer) > 5:
                        audio_data = np.concatenate(speech_buffer)
                        text = transcribe_speech(audio_data)

                        if text and len(text) > 1:
                            emit("transcript", text=text)
                        else:
                            emit("wake")  # No speech detected, listen again

                        # Reset for next utterance
                        is_speaking = False
                        speech_buffer = []
                        silence_counter = 0

                    # Timeout: max 30s recording
                    total_speech_time = sum(len(b) for b in speech_buffer) / SAMPLE_RATE
                    if total_speech_time > 30:
                        audio_data = np.concatenate(speech_buffer)
                        text = transcribe_speech(audio_data)
                        if text:
                            emit("transcript", text=text)
                        is_speaking = False
                        speech_buffer = []
                        silence_counter = 0

    except KeyboardInterrupt:
        pass
    except Exception as e:
        emit("error", message=str(e))


if __name__ == "__main__":
    main()
