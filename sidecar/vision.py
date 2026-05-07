#!/usr/bin/env python3
"""JARVIS v4 — Vision Module

Captures webcam frames and sends them to a Vision-Language Model
for desk/environment analysis.

Usage:
  python3 vision.py             # Capture + analyze, print JSON result
  python3 vision.py --capture   # Just capture frame to /tmp/jarvis_vision.jpg
"""

import json
import sys
import os
import base64
import urllib.request


def capture_frame(output_path: str = "/tmp/jarvis_vision.jpg") -> bool:
    """Capture a single frame from webcam using OpenCV or ffmpeg."""
    try:
        import cv2
        cap = cv2.VideoCapture(0)
        if not cap.isOpened():
            return False
        ret, frame = cap.read()
        cap.release()
        if ret:
            cv2.imwrite(output_path, frame)
            return True
        return False
    except ImportError:
        # Fallback: use ffmpeg
        import subprocess
        result = subprocess.run(
            ["ffmpeg", "-y", "-f", "v4l2", "-i", "/dev/video0",
             "-frames:v", "1", "-q:v", "2", output_path],
            capture_output=True, timeout=5
        )
        return result.returncode == 0


def analyze_frame(image_path: str, prompt: str = "Describe what you see on this desk. List all physical objects, devices, and their state.") -> str:
    """Send frame to a Vision-Language Model for analysis."""
    config = load_config()

    # Read and encode image
    with open(image_path, "rb") as f:
        image_data = base64.standard_b64encode(f.read()).decode("utf-8")

    # Try Anthropic Claude (supports vision)
    api_key = config.get("api_key", os.environ.get("ANTHROPIC_API_KEY", ""))
    base_url = config.get("base_url", "https://proxy.shift.aws.dev")
    model = config.get("model", "us.anthropic.claude-sonnet-4-6")

    body = json.dumps({
        "model": model,
        "max_tokens": 1024,
        "messages": [{
            "role": "user",
            "content": [
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/jpeg",
                        "data": image_data,
                    }
                },
                {"type": "text", "text": prompt}
            ]
        }]
    }).encode()

    req = urllib.request.Request(
        f"{base_url}/v1/messages",
        data=body,
        headers={
            "x-api-key": api_key,
            "anthropic-version": "2023-06-01",
            "content-type": "application/json",
        },
    )

    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            result = json.loads(resp.read())
            content = result.get("content", [])
            if content and content[0].get("type") == "text":
                return content[0]["text"]
            return "Could not analyze image"
    except Exception as e:
        return f"Vision error: {e}"


def load_config() -> dict:
    config_path = os.path.expanduser("~/.config/jarvis/config.json")
    try:
        with open(config_path) as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def main():
    if "--capture" in sys.argv:
        success = capture_frame()
        print(json.dumps({"success": success, "path": "/tmp/jarvis_vision.jpg"}))
        return

    # Full pipeline: capture + analyze
    if not capture_frame():
        print(json.dumps({"error": "Could not capture webcam frame"}))
        return

    description = analyze_frame("/tmp/jarvis_vision.jpg")

    # Extract objects from description
    result = {
        "description": description,
        "frame_path": "/tmp/jarvis_vision.jpg",
        "timestamp": __import__("datetime").datetime.now().isoformat(),
    }

    print(json.dumps(result))


if __name__ == "__main__":
    main()
