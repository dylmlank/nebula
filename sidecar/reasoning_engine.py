#!/usr/bin/env python3
"""JARVIS Reasoning Engine — Intent recognition + action planning.

Sends System State + User Input to Claude and gets back structured tool calls.
"""
import json
import sys
import os
import urllib.request

REASONING_PROMPT = """You are JARVIS, an agentic AI. You observe the user's environment and act.

SYSTEM STATE:
{system_state}

TOOLS: run_command, sudo, read_file, write_file, configure_workspace, vision_snapshot,
network, bluetooth, service, security_audit, google_search, calendar, notify, remember

RULES:
1. Analyze gap between current state and user goal
2. Generate tool calls to close the gap
3. Address user as "sir"
4. Be proactive — mention relevant system info

Respond JSON: {{"thought":"...","actions":[{{"tool":"name","args":{{"k":"v"}}}}],"response":"what to say"}}"""


def reason(user_input: str, system_state: dict) -> dict:
    config = load_config()
    state_text = format_state(system_state)
    prompt = REASONING_PROMPT.format(system_state=state_text)

    body = json.dumps({
        "model": config.get("model", "us.anthropic.claude-sonnet-4-6"),
        "max_tokens": 2048,
        "system": [{"type": "text", "text": prompt}],
        "messages": [{"role": "user", "content": [{"type": "text", "text": user_input}]}],
    }).encode()

    req = urllib.request.Request(
        f"{config.get('base_url', 'https://proxy.shift.aws.dev')}/v1/messages",
        data=body,
        headers={"x-api-key": config.get("api_key", ""), "anthropic-version": "2023-06-01", "content-type": "application/json"},
    )

    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            result = json.loads(resp.read())
            text = result.get("content", [{}])[0].get("text", "")
            return parse_response(text)
    except Exception as e:
        return {"thought": "", "actions": [], "response": f"Error: {e}"}


def parse_response(text: str) -> dict:
    try:
        if "{" in text:
            start = text.index("{")
            depth, end = 0, start
            for i, c in enumerate(text[start:], start):
                if c == "{": depth += 1
                elif c == "}": depth -= 1
                if depth == 0: end = i + 1; break
            return json.loads(text[start:end])
    except (json.JSONDecodeError, ValueError):
        pass
    return {"thought": "", "actions": [], "response": text}


def format_state(state: dict) -> str:
    parts = []
    if hw := state.get("hardware"):
        parts.append(f"CPU: {hw.get('cpu_usage', 0):.0f}% | RAM: {hw.get('memory_percent', 0):.0f}% | Disk: {hw.get('disk_free_gb', 0):.0f}G free")
    if procs := state.get("processes"):
        parts.append(f"Window: {procs.get('active_window', '?')}")
        if cats := procs.get("categories"):
            parts.append(f"Running: {', '.join(f'{k}:{len(v)}' for k,v in cats.items())}")
    if ws := state.get("workspace"):
        parts.append(f"Mode: {ws.get('current_mode', 'general')}")
    return "\n".join(parts)


def load_config() -> dict:
    try:
        with open(os.path.expanduser("~/.config/jarvis/config.json")) as f:
            return json.load(f)
    except: return {}


if __name__ == "__main__":
    if len(sys.argv) > 1:
        result = reason(sys.argv[1], {})
        print(json.dumps(result, indent=2))
    else:
        print("Usage: reasoning_engine.py 'user input'")
