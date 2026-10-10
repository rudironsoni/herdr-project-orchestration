#!/usr/bin/env python3
"""Drive a TUI on a pty. Script steps are JSON objects."""

import argparse
import fcntl
import json
import os
import pty
import re
import select
import signal
import sqlite3
import struct
import subprocess
import sys
import termios
import time

ANSI = re.compile(r"\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])")


def squashed(data):
    text = ANSI.sub("", data.decode("utf-8", "replace"))
    return re.sub(r"\s+", "", text)


def drive(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--rows", type=int, default=40)
    parser.add_argument("--cols", type=int, default=160)
    parser.add_argument("--script", required=True)
    parser.add_argument("--capture", required=True)
    parser.add_argument("--pid", required=True)
    parser.add_argument("cmd", nargs=argparse.REMAINDER)
    ns = parser.parse_args(argv)
    cmd = ns.cmd[1:] if ns.cmd and ns.cmd[0] == "--" else ns.cmd
    master, slave = pty.openpty()
    winsize = struct.pack("HHHH", ns.rows, ns.cols, 0, 0)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, winsize)
    proc = subprocess.Popen(
        cmd,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        start_new_session=True,
        close_fds=True,
    )
    os.close(slave)
    with open(ns.pid, "w", encoding="utf-8") as handle:
        handle.write(str(proc.pid))
    steps = json.loads(open(ns.script, encoding="utf-8").read())
    buf = bytearray()
    capture = open(ns.capture, "wb")

    def pump(seconds):
        end = time.time() + seconds
        while time.time() < end:
            if proc.poll() is not None:
                return False
            ready, _, _ = select.select([master], [], [], 0.05)
            if not ready:
                continue
            try:
                chunk = os.read(master, 65536)
            except OSError:
                return False
            if not chunk:
                return False
            buf.extend(chunk)
            if not capture.closed:
                capture.write(chunk)
                capture.flush()
        return proc.poll() is None

    def stop():
        if proc.poll() is not None:
            return
        for sig, name in ((signal.SIGTERM, "TERM"), (signal.SIGKILL, "KILL")):
            if proc.poll() is not None:
                return
            try:
                os.killpg(proc.pid, sig)
            except OSError:
                try:
                    os.kill(proc.pid, sig)
                except OSError:
                    subprocess.run(
                        ["/bin/kill", f"-{name}", "--", str(proc.pid)],
                        check=False,
                    )
            deadline = time.time() + 1
            while time.time() < deadline and proc.poll() is None:
                pump(0.1)

    def fail(code, detail):
        capture.close()
        print(detail, file=sys.stderr)
        print(squashed(buf)[-3000:], file=sys.stderr)
        stop()
        if proc.poll() is None:
            print("tui still running", file=sys.stderr)
        raise SystemExit(code)

    for step in steps:
        if "wait" in step:
            needle = step["wait"]
            deadline = time.time() + step.get("timeout", 5)
            while needle not in squashed(buf):
                if time.time() > deadline or not pump(0.2):
                    fail(2, f"missing {needle}")
        elif "send" in step:
            try:
                os.write(master, step["send"].encode())
            except OSError:
                if proc.poll() is not None:
                    break
                raise
            pump(0.08)
        elif "wait_file" in step:
            path = step["wait_file"]
            deadline = time.time() + step.get("timeout", 15)
            while not os.path.exists(path):
                if time.time() > deadline:
                    fail(3, f"missing file {path}")
                pump(0.1)
        elif "hold" in step:
            pump(step["hold"])
    capture.close()
    stop()
    if proc.poll() is None:
        print("tui still running", file=sys.stderr)
        raise SystemExit(4)
    proc.wait()


def inspect(db):
    con = sqlite3.connect(db)
    threads = []
    for thread_id, body in con.execute("SELECT id, body FROM threads"):
        data = json.loads(body)
        threads.append(
            {
                "id": data.get("id", thread_id),
                "row_id": thread_id,
                "status": data.get("status"),
                "kind": data.get("kind"),
                "worktree_path": data.get("worktree_path", ""),
                "branch": data.get("branch", ""),
                "pane_id": data.get("pane_id", ""),
                "title": data.get("title", ""),
            }
        )
    ops = []
    for op_id, status, payload, error in con.execute(
        "SELECT id, status, payload, error FROM operations WHERE kind = 'thread_start'"
    ):
        ops.append(
            {"id": op_id, "status": status, "payload": payload, "error": error}
        )
    print(json.dumps({"threads": threads, "operations": ops}))


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "inspect":
        inspect(sys.argv[2])
        return
    if len(sys.argv) > 1 and sys.argv[1] == "drive":
        drive(sys.argv[2:])
        return
    raise SystemExit("usage: pty_drive.py drive|inspect")


if __name__ == "__main__":
    main()
