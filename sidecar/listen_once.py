#!/usr/bin/env python3
"""Nebula one-shot STT — record a single utterance and print the transcript.

Used by Nebula Island's mic button: record until the user stops talking
(silence-gated), transcribe once with faster-whisper, print the text, exit.

Output protocol (one JSON object per line on stdout):
  {"type": "listening"}              — mic is live, speak now
  {"type": "transcript", "text": ...} — final transcription
  {"type": "error", "message": ...}   — something went wrong

Kept separate from voice_sidecar.py (the wake-word loop) on purpose: this is a
short-lived, single-shot process so the island stays simple and responsive.
"""
import sys
import json
import time


def emit(t, **kw):
    print(json.dumps({"type": t, **kw}), flush=True)


def main():
    try:
        import numpy as np
        import sounddevice as sd
        from faster_whisper import WhisperModel
    except Exception as e:  # noqa: BLE001
        emit("error", message=f"missing deps: {e}")
        return

    sample_rate = 16000
    frame = int(sample_rate * 0.1)          # 100ms frames
    max_seconds = 12                         # hard cap once speaking
    no_speech_timeout = 5.0                  # give up if nothing is said
    silence_limit = 1.0                      # stop after this much trailing silence
    # RMS threshold for "is someone talking" — tuned for typical mic gain.
    speech_rms = 0.012

    # tiny model: fast, good enough for short commands/questions
    try:
        model = WhisperModel("base", device="cpu", compute_type="int8")
    except Exception:  # noqa: BLE001
        model = WhisperModel("tiny", device="cpu", compute_type="int8")

    chunks = []
    started = False
    silent_for = 0.0
    speech_secs = 0.0    # time elapsed since speech began
    waited = 0.0         # time spent waiting for speech to begin

    emit("listening")
    try:
        with sd.InputStream(samplerate=sample_rate, channels=1, dtype="float32") as stream:
            while True:
                data, _ = stream.read(frame)
                mono = data[:, 0]
                rms = float(np.sqrt(np.mean(mono ** 2)) + 1e-9)

                if rms > speech_rms:
                    started = True
                    silent_for = 0.0
                    chunks.append(mono.copy())
                elif started:
                    silent_for += 0.1
                    chunks.append(mono.copy())
                    if silent_for >= silence_limit:
                        break  # natural end of utterance
                else:
                    waited += 0.1
                    if waited >= no_speech_timeout:
                        break  # nobody spoke

                if started:
                    speech_secs += 0.1
                    if speech_secs >= max_seconds:
                        break  # hard cap
    except Exception as e:  # noqa: BLE001
        emit("error", message=f"mic error: {e}")
        return

    if not chunks:
        emit("transcript", text="")
        return

    audio = np.concatenate(chunks).astype("float32")
    try:
        segments, _ = model.transcribe(audio, language="en", beam_size=1)
        text = " ".join(s.text for s in segments).strip()
    except Exception as e:  # noqa: BLE001
        emit("error", message=f"transcribe error: {e}")
        return

    emit("transcript", text=text)


if __name__ == "__main__":
    main()
