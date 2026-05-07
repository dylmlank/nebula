#!/usr/bin/env python3
"""JARVIS Vision — webcam capture + VLM desk analysis."""
import json, sys, os, base64, urllib.request


def capture_frame(path="/tmp/jarvis_vision.jpg") -> bool:
    try:
        import cv2
        cap = cv2.VideoCapture(0)
        if not cap.isOpened(): return False
        ret, frame = cap.read()
        cap.release()
        if ret: cv2.imwrite(path, frame); return True
        return False
    except ImportError:
        import subprocess
        return subprocess.run(["ffmpeg", "-y", "-f", "v4l2", "-i", "/dev/video0", "-frames:v", "1", "-q:v", "2", path], capture_output=True, timeout=5).returncode == 0


def analyze_frame(path="/tmp/jarvis_vision.jpg", prompt="Describe what you see. List all objects.") -> str:
    config = load_config()
    with open(path, "rb") as f:
        img = base64.standard_b64encode(f.read()).decode()

    body = json.dumps({
        "model": config.get("model", "us.anthropic.claude-sonnet-4-6"),
        "max_tokens": 1024,
        "messages": [{"role": "user", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": img}},
            {"type": "text", "text": prompt}
        ]}]
    }).encode()

    req = urllib.request.Request(
        f"{config.get('base_url', 'https://proxy.shift.aws.dev')}/v1/messages",
        data=body,
        headers={"x-api-key": config.get("api_key", ""), "anthropic-version": "2023-06-01", "content-type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return json.loads(resp.read()).get("content", [{}])[0].get("text", "")
    except Exception as e:
        return f"Vision error: {e}"


def load_config():
    try:
        with open(os.path.expanduser("~/.config/jarvis/config.json")) as f: return json.load(f)
    except: return {}


if __name__ == "__main__":
    if not capture_frame():
        print(json.dumps({"error": "Could not capture frame"}))
    else:
        desc = analyze_frame()
        print(json.dumps({"description": desc, "path": "/tmp/jarvis_vision.jpg"}))
