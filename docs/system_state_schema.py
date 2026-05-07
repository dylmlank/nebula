"""JARVIS v4 — System State Schema

The System State is a JSON snapshot of the user's environment that gets
injected into the reasoning engine's context. It enables JARVIS to understand
what's happening on the machine and make intelligent decisions.
"""

SYSTEM_STATE_SCHEMA = {
    "timestamp": "ISO 8601 datetime",
    "hardware": {
        "cpu": {
            "usage_percent": "float (0-100)",
            "cores": "int",
            "frequency_mhz": "float",
        },
        "memory": {
            "total_gb": "float",
            "used_gb": "float",
            "available_gb": "float",
            "usage_percent": "float",
        },
        "gpu": {
            "name": "string or null",
            "usage_percent": "float or null",
            "vram_used_mb": "float or null",
        },
        "disk": {
            "total_gb": "float",
            "used_gb": "float",
            "free_gb": "float",
        },
        "network": {
            "interfaces": ["name: ip"],
            "bytes_sent": "int",
            "bytes_recv": "int",
        },
    },
    "peripherals": {
        "microphone_active": "bool",
        "webcam_active": "bool",
        "displays": "int",
        "bluetooth_connected": ["device names"],
    },
    "processes": {
        "active_window": "string (window title)",
        "running": [
            {
                "name": "string",
                "pid": "int",
                "cpu_percent": "float",
                "memory_mb": "float",
            }
        ],
        "categories": {
            "browsers": ["chrome", "firefox"],
            "editors": ["code", "nvim"],
            "terminals": ["alacritty", "kitty"],
            "media": ["spotify", "vlc"],
            "comms": ["discord", "slack"],
            "dev_tools": ["docker", "node"],
        },
    },
    "workspace": {
        "current_mode": "string (coding/streaming/general/creative)",
        "open_projects": ["directory paths"],
        "git_status": {
            "branch": "string",
            "dirty": "bool",
            "ahead": "int",
        },
    },
    "vision": {
        "last_snapshot": "ISO datetime or null",
        "detected_objects": ["list of physical objects on desk"],
        "desk_state": "string description",
    },
}
