#!/usr/bin/env python3
"""JARVIS Workspace Configurator — dynamic window layouts."""
import json, subprocess, sys, time

PRESETS = {
    "coding": {"apps": ["code", "alacritty"], "layout": [("code", "left"), ("alacritty", "right")], "close": ["discord", "spotify"]},
    "streaming": {"apps": ["obs"], "layout": [("obs", "maximize")], "close": []},
    "creative": {"apps": ["blender"], "layout": [("blender", "maximize")], "close": ["discord"]},
    "focus": {"apps": [], "layout": [], "close": ["discord", "slack", "spotify", "firefox"]},
    "research": {"apps": ["firefox", "code"], "layout": [("firefox", "left"), ("code", "right")], "close": []},
}

def configure(mode: str, custom_apps=None) -> dict:
    preset = PRESETS.get(mode, {"apps": [], "layout": [], "close": []})
    results = []

    for app in preset["close"]:
        if subprocess.run(["pkill", "-f", app], capture_output=True).returncode == 0:
            results.append(f"Closed {app}")

    for app in (custom_apps or preset["apps"]):
        if subprocess.run(["pgrep", "-f", app], capture_output=True).returncode != 0:
            subprocess.Popen([app], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            results.append(f"Launched {app}")
            time.sleep(0.5)

    time.sleep(1)
    try:
        out = subprocess.check_output(["xdotool", "getdisplaygeometry"], text=True).strip()
        w, h = [int(x) for x in out.split()]
    except: w, h = 1920, 1080

    positions = {"left": (0,0,w//2,h), "right": (w//2,0,w//2,h), "maximize": (0,0,w,h), "top-left": (0,0,w//2,h//2), "bottom-right": (w//2,h//2,w//2,h//2)}

    for app, pos in preset["layout"]:
        try:
            wid = subprocess.check_output(["xdotool", "search", "--name", app], text=True).strip().split("\n")[0]
            if wid and pos in positions:
                x, y, pw, ph = positions[pos]
                subprocess.run(["xdotool", "windowmove", wid, str(x), str(y), "windowsize", wid, str(pw), str(ph)], capture_output=True)
                results.append(f"{app} → {pos}")
        except: pass

    return {"mode": mode, "actions": results}

if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "coding"
    print(json.dumps(configure(mode, sys.argv[2:] or None)))
