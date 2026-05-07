#!/usr/bin/env python3
"""JARVIS v4 — Reasoning Engine (Agentic Orchestrator)

Instead of mapping keywords to tools, this sends the System State + User Intent
to the LLM and lets it reason about what actions to take.

The loop:
1. Observe: Get system state from Rust backend
2. Orient: Combine state + user input + memory into context
3. Decide: LLM generates a plan (sequence of tool calls)
4. Act: Execute tools, feed results back
5. Repeat until goal is achieved

Protocol (stdin JSON lines from Tauri):
  {"type": "user_input", "text": "...", "system_state": {...}}
  {"type": "shutdown"}

Protocol (stdout JSON lines to Tauri):
  {"type": "thinking", "thought": "..."}
  {"type": "action", "tool": "...", "args": {...}}
  {"type": "response", "text": "..."}
  {"type": "workspace_change", "mode": "...", "layout": {...}}
  {"type": "render_hologram", "model": "...", "context": {...}}
"""

import json
import sys
import os
from typing import Optional

# Reasoning system prompt — tells the LLM HOW to think
REASONING_PROMPT = """You are JARVIS, an agentic AI system. You observe the user's environment and act autonomously.

SYSTEM STATE (current environment):
{system_state}

AVAILABLE TOOLS:
- run_command: Execute shell command. Args: command
- sudo: Run with elevated privileges. Args: command
- configure_workspace: Set up window layout. Args: mode (coding/streaming/creative/focus), apps (list)
- vision_snapshot: Capture webcam frame and analyze desk. No args.
- network: Network operations. Args: action (scan/ports/wifi/interfaces), target
- bluetooth: Bluetooth control. Args: action (scan/connect/disconnect), device
- service: System services. Args: action (start/stop/status/list), service
- open_app: Launch application. Args: name
- move_window: Position a window. Args: app, position (left/right/top/bottom/center/maximize)
- notify: Desktop notification. Args: message
- remember: Store fact. Args: fact
- web_search: Search the web. Args: query

RULES:
1. Analyze the gap between CURRENT STATE and USER GOAL
2. Generate a PLAN as a sequence of tool calls
3. Address the user as "sir"
4. Be proactive — if you notice something relevant in the system state, mention it
5. For workspace changes, use configure_workspace with appropriate mode
6. If physical objects are relevant, trigger vision_snapshot first

Respond with a JSON object:
{
  "thought": "Brief reasoning about what needs to happen",
  "actions": [{"tool": "name", "args": {"key": "value"}}],
  "response": "What to say to the user"
}
"""


def reason(user_input: str, system_state: dict) -> dict:
    """Send to LLM for reasoning. Returns structured action plan."""

    # Format system state for context
    state_summary = format_state(system_state)

    prompt = REASONING_PROMPT.format(system_state=state_summary)

    # Build messages
    messages = [
        {"role": "system", "content": prompt},
        {"role": "user", "content": user_input},
    ]

    # Call the LLM (uses same Anthropic proxy as main brain)
    config = load_config()
    response = call_llm(messages, config)

    # Parse structured response
    try:
        # Try to extract JSON from response
        if "```json" in response:
            json_str = response.split("```json")[1].split("```")[0]
        elif "{" in response:
            # Find the JSON object
            start = response.index("{")
            depth = 0
            end = start
            for i, c in enumerate(response[start:], start):
                if c == "{": depth += 1
                elif c == "}": depth -= 1
                if depth == 0:
                    end = i + 1
                    break
            json_str = response[start:end]
        else:
            return {"thought": "", "actions": [], "response": response}

        plan = json.loads(json_str)
        return {
            "thought": plan.get("thought", ""),
            "actions": plan.get("actions", []),
            "response": plan.get("response", response),
        }
    except (json.JSONDecodeError, ValueError):
        return {"thought": "", "actions": [], "response": response}


def format_state(state: dict) -> str:
    """Format system state into a readable context block."""
    parts = []

    if hw := state.get("hardware"):
        parts.append(f"CPU: {hw.get('cpu_usage', 0):.0f}% ({hw.get('cpu_cores', 0)} cores)")
        parts.append(f"RAM: {hw.get('memory_used_gb', 0):.1f}/{hw.get('memory_total_gb', 0):.1f}GB ({hw.get('memory_percent', 0):.0f}%)")
        parts.append(f"Disk: {hw.get('disk_free_gb', 0):.0f}GB free")

    if procs := state.get("processes"):
        parts.append(f"Active window: {procs.get('active_window', 'unknown')}")
        if cats := procs.get("categories"):
            active = [f"{k}: {', '.join(v)}" for k, v in cats.items()]
            parts.append(f"Running: {'; '.join(active)}")

    if ws := state.get("workspace"):
        parts.append(f"Mode: {ws.get('current_mode', 'general')}")
        if projects := ws.get("open_projects"):
            parts.append(f"Projects: {', '.join(projects)}")

    if periph := state.get("peripherals"):
        periph_status = []
        if periph.get("microphone_active"): periph_status.append("mic ON")
        if periph.get("webcam_active"): periph_status.append("webcam ON")
        if bt := periph.get("bluetooth_devices"):
            periph_status.append(f"BT: {', '.join(bt)}")
        if periph_status:
            parts.append(f"Peripherals: {', '.join(periph_status)}")

    return "\n".join(parts)


def load_config() -> dict:
    """Load JARVIS config."""
    config_path = os.path.expanduser("~/.config/jarvis/config.json")
    try:
        with open(config_path) as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def call_llm(messages: list, config: dict) -> str:
    """Call the Anthropic API (same proxy as brain.rs)."""
    import urllib.request

    api_key = config.get("api_key", os.environ.get("ANTHROPIC_API_KEY", ""))
    base_url = config.get("base_url", "https://proxy.shift.aws.dev")
    model = config.get("model", "us.anthropic.claude-sonnet-4-6")

    # Convert to Anthropic format
    system_msg = ""
    api_messages = []
    for msg in messages:
        if msg["role"] == "system":
            system_msg = msg["content"]
        else:
            api_messages.append({"role": msg["role"], "content": [{"type": "text", "text": msg["content"]}]})

    body = json.dumps({
        "model": model,
        "max_tokens": 2048,
        "system": [{"type": "text", "text": system_msg}],
        "messages": api_messages,
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
            return ""
    except Exception as e:
        return f"Error calling LLM: {e}"


def main():
    """Main reasoning loop — reads from stdin, writes to stdout."""
    emit("ready")

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue

        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue

        if msg.get("type") == "shutdown":
            break

        if msg.get("type") == "user_input":
            text = msg.get("text", "")
            system_state = msg.get("system_state", {})

            emit("thinking", thought="Analyzing environment and planning actions...")

            # Run reasoning
            plan = reason(text, system_state)

            # Emit thought
            if plan.get("thought"):
                emit("thinking", thought=plan["thought"])

            # Emit actions for Tauri to execute
            for action in plan.get("actions", []):
                emit("action", tool=action.get("tool", ""), args=action.get("args", {}))

            # Emit final response
            if plan.get("response"):
                emit("response", text=plan["response"])


def emit(event_type: str, **kwargs):
    """Send event to Tauri app."""
    data = {"type": event_type, **kwargs}
    print(json.dumps(data), flush=True)


if __name__ == "__main__":
    main()
