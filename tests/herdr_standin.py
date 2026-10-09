#!/usr/bin/env python3
"""Proxy a disposable herdr. Pause after a real worktree create returns."""

import json
import os
import subprocess
import sys
import time

REAL = os.environ["HPO_REAL_HERDR"]
LOG = os.environ["HPO_HERDR_LOG"]
MODE = os.environ["HPO_HERDR_MODE"]
STATE = os.environ["HPO_HERDR_STATE"]
MARKER = os.environ.get("HPO_WT_MARKER", "")
FOCUS = os.environ.get("HPO_FOCUS_MARKER", "")
PROJECT = os.environ.get("HPO_PROJECT_DIR", "")


def log(obj):
    with open(LOG, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(obj) + "\n")


def emit(payload, code=0):
    text = json.dumps(payload)
    stream = sys.stdout if code == 0 else sys.stderr
    stream.write(text)
    log({"argv": sys.argv[1:], "exit": code, "out": text[:2000], "delegated": False})
    raise SystemExit(code)


def load_state():
    if not os.path.exists(STATE):
        return {}
    with open(STATE, encoding="utf-8") as handle:
        return json.load(handle)


def save_state(state):
    with open(STATE, "w", encoding="utf-8") as handle:
        json.dump(state, handle)


def current_mode():
    if not os.path.exists(MODE):
        return "idle"
    return open(MODE, encoding="utf-8").read().strip() or "idle"


def delegate(args):
    proc = subprocess.run([REAL, *args], capture_output=True, text=True, check=False)
    log(
        {
            "argv": args,
            "exit": proc.returncode,
            "out": proc.stdout[:4000],
            "err": proc.stderr[:2000],
            "delegated": True,
        }
    )
    return proc


def pass_through(proc):
    sys.stdout.write(proc.stdout)
    sys.stderr.write(proc.stderr)
    raise SystemExit(proc.returncode)


def root_pane(stdout):
    try:
        data = json.loads(stdout)
    except json.JSONDecodeError:
        return None
    result = data.get("result", data)
    if not isinstance(result, dict):
        return None
    pane = result.get("root_pane")
    if not isinstance(pane, dict) or not pane.get("pane_id"):
        return None
    return pane


def agent(status):
    state = load_state()
    return {
        "pane_id": state.get("pane_id", ""),
        "tab_id": state.get("tab_id", ""),
        "workspace_id": state.get("workspace_id", ""),
        "cwd": PROJECT,
        "name": state.get("name", "hpc-accept"),
        "agent": "claude",
        "agent_status": status,
        "terminal_id": state.get("terminal_id", ""),
        "state_change_seq": 2,
    }


def status_for(mode):
    if mode == "blocked":
        return "blocked"
    if mode == "unknown":
        return "unknown"
    return "idle"


args = sys.argv[1:]
kind = args[:2]

# Live mode forwards agent calls. Herdr's own reply is the observation.
if current_mode() == "live" and kind in (
    ["agent", "list"],
    ["agent", "start"],
    ["agent", "prompt"],
    ["agent", "get"],
    ["agent", "read"],
    ["agent", "send-keys"],
    ["agent", "wait"],
):
    pass_through(delegate(args))

if kind == ["agent", "list"]:
    mode = current_mode()
    state = load_state()
    if mode == "missing" or not state.get("pane_id"):
        emit({"result": {"agents": []}})
    agents = [agent(status_for(mode))]
    if mode == "stale":
        decoy = agent("idle")
        decoy["pane_id"] = "w9:p9"
        decoy["tab_id"] = "w9:t9"
        decoy["workspace_id"] = "w9"
        decoy["name"] = "other"
        decoy["state_change_seq"] = 99
        agents.append(decoy)
    emit({"result": {"agents": agents}})

if kind == ["agent", "start"]:
    state = load_state()
    if len(args) > 2:
        state["name"] = args[2]
        save_state(state)
    emit({"result": {"agent": agent("idle")}})

if kind == ["agent", "prompt"]:
    mode = current_mode()
    if mode == "uncertain":
        emit(
            {
                "error": {
                    "code": "agent_prompt_stalled",
                    "message": "agent did not start working",
                }
            },
            1,
        )
    if mode == "refused":
        emit({"error": {"code": "agent_blocked", "message": "blocked"}}, 1)
    emit({"result": {"ok": True}})

if kind == ["agent", "focus"]:
    proc = delegate(args)
    if FOCUS:
        with open(FOCUS, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "argv": args,
                    "exit": proc.returncode,
                    "out": proc.stdout,
                    "err": proc.stderr,
                },
                handle,
            )
    pass_through(proc)

if kind == ["workspace", "create"]:
    proc = delegate(args)
    pane = root_pane(proc.stdout)
    if pane:
        state = load_state()
        state["workspace_id"] = pane.get("workspace_id", "")
        state["tab_id"] = pane.get("tab_id", "")
        state["pane_id"] = pane.get("pane_id", "")
        state["terminal_id"] = pane.get("terminal_id", "")
        save_state(state)
    pass_through(proc)

if kind == ["worktree", "create"]:
    proc = delegate(args)
    if proc.returncode == 0 and MARKER:
        with open(MARKER, "w", encoding="utf-8") as handle:
            handle.write(proc.stdout)
        time.sleep(30)
    pass_through(proc)

pass_through(delegate(args))
