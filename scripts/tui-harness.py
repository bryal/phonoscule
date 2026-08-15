#!/usr/bin/env python3
"""Runs the TUI on a pseudo-terminal of an exact size, for measuring what it costs to draw.

    scripts/tui-harness.py --cols 200 --rows 50 -- ./target/profiling/phonoscule-tui conf.toml

Prints `pid <N>` once the player is up, then drains the terminal until killed.

Under a tiling compositor the window size is the compositor's decision, so a pty is the only way to
ask for one and get it - and a tall terminal against a short one is how you find out whether a frame
costs what is on screen or what is in the library. It also leaves the terminal emulator out of the
measurement; the emulator's own share is a separate run in a real one.

The drain is not optional: a player nobody reads blocks in `write`, and would measure as free.
"""

import argparse
import fcntl
import os
import pty
import select
import signal
import struct
import sys
import termios
import time


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cols", type=int, default=200)
    ap.add_argument("--rows", type=int, default=50)
    # Keystrokes to send once the player is up, as a Python string escape: "\t" for Tab to switch
    # views, " " to play or pause, "\x01" for Ctrl+A to queue everything shown.
    ap.add_argument("--keys", default="")
    ap.add_argument("--keys-after", type=float, default=3.0,
                    help="seconds to wait before sending keys, so the boot scan has settled")
    ap.add_argument("--key-delay", type=float, default=0.4,
                    help="seconds between keystrokes")
    ap.add_argument("--tee", default=None,
                    help="write what the player draws to this file, to check a configuration is the "
                         "one it was meant to be")
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    args = ap.parse_args()

    cmd = args.cmd[1:] if args.cmd and args.cmd[0] == "--" else args.cmd
    if not cmd:
        ap.error("expected a command after --")

    master, slave = pty.openpty()
    # What the player reads back through its own ioctl, and lays every frame out against.
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", args.rows, args.cols, 0, 0))

    pid = os.fork()
    if pid == 0:
        os.close(master)
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        for fd in (0, 1, 2):
            os.dup2(slave, fd)
        if slave > 2:
            os.close(slave)
        os.execvp(cmd[0], cmd)
        os._exit(127)

    os.close(slave)
    print(f"pid {pid}", flush=True)

    stop = False

    def handle(_signum, _frame):
        nonlocal stop
        stop = True

    signal.signal(signal.SIGTERM, handle)
    signal.signal(signal.SIGINT, handle)

    tee = open(args.tee, "wb") if args.tee else None
    keys = args.keys.encode().decode("unicode_escape").encode() if args.keys else b""
    keys_at = time.monotonic() + args.keys_after
    sent = 0

    while not stop:
        if os.waitpid(pid, os.WNOHANG)[0] == pid:
            break
        if sent < len(keys) and time.monotonic() >= keys_at:
            os.write(master, keys[sent:sent + 1])
            sent += 1
            keys_at = time.monotonic() + args.key_delay
        try:
            # Short timeout so the keystroke schedule is not stalled. Large read: a halfblocks cover
            # repaint is tens of kilobytes a frame.
            ready, _, _ = select.select([master], [], [], 0.1)
            if ready:
                chunk = os.read(master, 1 << 16)
                if not chunk:
                    break
                if tee is not None:
                    tee.write(chunk)
        except OSError:
            break

    if tee is not None:
        tee.close()

    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.kill(pid, sig)
            for _ in range(20):
                if os.waitpid(pid, os.WNOHANG)[0] == pid:
                    return
                time.sleep(0.05)
        except ProcessLookupError:
            return


if __name__ == "__main__":
    sys.exit(main())
