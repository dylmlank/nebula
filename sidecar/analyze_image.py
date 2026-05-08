#!/usr/bin/env python3
"""JARVIS — Analyze any image with Claude Vision.

Usage: python3 analyze_image.py <image_path> [prompt]
"""
import json
import sys
import os
import base64
import urllib.request


def analyze(image_path: str, prompt: str = "Describe what you see.") -> str:
    config = load_config()
    api_key = config.get("api_key", os.environ.get("ANTHROPIC_API_KEY", ""))
    base_url = config.get("base_url", "https://proxy.shift.aws.dev")
    model = config.get("model", "us.anthropic.claude-sonnet-4-6")

    # Read and encode image
    with open(image_path, "rb") as f:
        img_data = base64.standard_b64encode(f.read()).decode("utf-8")

    # Detect media type
    ext = image_path.lower().rsplit(".", 1)[-1]
    media_types = {"png": "image/png", "jpg": "image/jpeg", "jpeg": "image/jpeg", "webp": "image/webp", "gif": "image/gif"}
    media_type = media_types.get(ext, "image/png")

    body = json.dumps({
        "model": model,
        "max_tokens": 1024,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "image", "source": {"type": "base64", "media_type": media_type, "data": img_data}},
                {"type": "text", "text": prompt},
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
            return "Could not analyze image."
    except Exception as e:
        return f"Error: {e}"


def load_config() -> dict:
    try:
        with open(os.path.expanduser("~/.config/jarvis/config.json")) as f:
            return json.load(f)
    except:
        return {}


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: analyze_image.py <image_path> [prompt]")
        sys.exit(1)

    image_path = sys.argv[1]
    prompt = sys.argv[2] if len(sys.argv) > 2 else "Describe what you see in detail."

    if not os.path.exists(image_path):
        print(f"File not found: {image_path}")
        sys.exit(1)

    result = analyze(image_path, prompt)
    print(result)
