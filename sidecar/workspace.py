#!/usr/bin/env python3
"""JARVIS v4 — Workspace Configurator

Dynamically configures window layouts and launches app suites
based on workspace mode.

Modes:
  coding    — editor left, terminal right, browser minimized
  streaming — OBS top-left, chat bottom-right, browser floating
  creative  — blender/gimp maximized, reference images tiled
  focus     — close distractions, single app maximized
  research  — browser left, notes right
"""

import json
import subprocess
import sys
import time


WORKSPACE_PRESETS = {
    "coding": {
        "apps": ["code", "alacritty"],
        "layout": [
            {"app": "code", "position": "left"},
            {"app": "alacritty", "position": "right"},
        ],
        "close": ["discord", "slack", "spotify"],
    },
    "streaming": {
        "apps": ["obs", "firefox"],
        "layout": [
            {"app": "obs", "position": "top-left"},
            {"app": "firefox", "position": "bottom-right"},
        ],
        "close": [],
    },
    "creative": {
        "apps": ["blender"],
        "layout": [
            {"app": "blender", "position": "maximize"},
        ],
        "close": ["discord", "slack"],
    },
    "focus": {
        "apps": [],
        "layout": [],
        "close": ["discord", "slack", "spotify", "firefox", "chrome"],
    },
    "research": {
        "apps": ["firefox", "code"],
        "layout": [
            {"app": "firefox", "position": "left"},
            {"app": "code", "position": "right"},
        ],
        "close": [],
    },
}


def configure_workspace(mode: str, custom_apps: list = None) -> dict:
    """Set up workspace for given mode."""
    preset = WORKSPACE_PRESETS.get(mode, WORKSPACE_PRESETS["general"] if "general" in WORKSPACE_PRESETS else {"apps": [], "layout": [], "close": []})

    results = {"mode": mode, "actions": []}

    # Close distracting apps
    for app in preset.get("close", []):
        r = subprocess.run(["pkill", "-f", app], capture_output=True)
        if r.returncode == 0:
            results["actions"].append(f"Closed {app}")

    # Launch required apps
    apps_to_launch = custom_apps or preset.get("apps", [])
    for app in apps_to_launch:
        if not is_running(app):
            subprocess.Popen(
                [app], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                start_new_session=True
            )
            results["actions"].append(f"Launched {app}")
            time.sleep(0.5)

    # Apply window layout
    time.sleep(1)  # Let apps start
    for layout in preset.get("layout", []):
        position_window(layout["app"], layout["position"])
        results["actions"].append(f"Positioned {layout['app']} → {layout['position']}")

    return results


def is_running(app: str) -> bool:
    """Check if an app is already running."""
    r = subprocess.run(["pgrep", "-f", app], capture_output=True)
    return r.returncode == 0


def position_window(app: str, position: str):
    """Position a window using wmctrl or xdotool."""
    # Get screen dimensions
    try:
        output = subprocess.check_output(
            ["xdotool", "getdisplaygeometry"], text=True
        ).strip()
        w, h = [int(x) for x in output.split()]
    except Exception:
        w, h = 1920, 1080

    # Find window by app name
    try:
        wid = subprocess.check_output(
            ["xdotool", "search", "--name", app], text=True
        ).strip().split("\n")[0]
    except Exception:
        return

    if not wid:
        return

    positions = {
        "left": (0, 0, w // 2, h),
        "right": (w // 2, 0, w // 2, h),
        "top-left": (0, 0, w // 2, h // 2),
        "top-right": (w // 2, 0, w // 2, h // 2),
        "bottom-left": (0, h // 2, w // 2, h // 2),
        "bottom-right": (w // 2, h // 2, w // 2, h // 2),
        "maximize": (0, 0, w, h),
        "center": (w // 4, h // 4, w // 2, h // 2),
    }

    if position in positions:
        x, y, pw, ph = positions[position]
        subprocess.run([
            "xdotool", "windowactivate", wid,
            "windowmove", wid, str(x), str(y),
            "windowsize", wid, str(pw), str(ph),
        ], capture_output=True)


def main():
    if len(sys.argv) < 2:
        print(json.dumps({"error": "Usage: workspace.py <mode> [apps...]"}))
        return

    mode = sys.argv[1]
    custom_apps = sys.argv[2:] if len(sys.argv) > 2 else None

    result = configure_workspace(mode, custom_apps)
    print(json.dumps(result))


if __name__ == "__main__":
    main()
