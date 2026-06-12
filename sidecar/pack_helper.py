#!/usr/bin/env python3
"""Puppy Pack coordination helper -- for AI agents working in a pack.

Reads the pack breadcrumb at ./.puppy/pack.json (drop-dir overridable with
--dir) and talks to the pack relay with one-shot TCP connections. No third-
party dependencies; safe to run from any workspace.

Commands:
  claim <path> [--note TEXT]   claim a file before editing it (fails if a
                               teammate holds it; re-claiming refreshes yours)
  release <path>               release your claim when you're done
  claims                       list the room's active claims
  post <message>               post a line to the pack chat (announce plans)
  status                       show teammates + their puppies' activity

Exit code 0 = success; 1 = refused (e.g. file already claimed) or error.
"""

import argparse
import json
import os
import socket
import sys


def load_breadcrumb(directory: str) -> dict:
    path = os.path.join(directory, ".puppy", "pack.json")
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception as exc:
        sys.exit(f"not in an active pack (couldn't read {path}: {exc})")


def rpc(relay: str, msg: dict) -> dict:
    """One-shot: connect, send one line, read one reply line."""
    host, _, port = relay.rpartition(":")
    if not host:
        host, port = relay, "9220"
    try:
        with socket.create_connection((host, int(port)), timeout=10) as sock:
            sock.sendall((json.dumps(msg) + "\n").encode("utf-8"))
            reply = sock.makefile("r", encoding="utf-8").readline()
        return json.loads(reply)
    except Exception as exc:
        sys.exit(f"relay unreachable at {relay}: {exc}")


def describe_claim(c: dict) -> str:
    who = c.get("user", "?")
    pup = c.get("puppy") or ""
    note = c.get("note") or ""
    tag = f"{who} ({pup})" if pup else who
    return f"{c.get('path', '?')} -- claimed by {tag}" + (f": {note}" if note else "")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dir", default=".", help="workspace root (default: cwd)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("claim", help="claim a file before editing it")
    p.add_argument("path")
    p.add_argument("--note", default="", help="why you're claiming it")
    p = sub.add_parser("release", help="release your claim")
    p.add_argument("path")
    sub.add_parser("claims", help="list active claims")
    p = sub.add_parser("post", help="post to the pack chat")
    p.add_argument("message")
    sub.add_parser("status", help="teammates + activity (from the breadcrumb)")
    args = ap.parse_args()

    bc = load_breadcrumb(args.dir)
    relay = bc.get("relay", "")
    room = bc.get("room", "")
    user = bc.get("user", "")
    puppy = bc.get("puppy", "")
    if not (relay and room):
        sys.exit("pack breadcrumb has no relay/room (rejoin the pack in puppy-home)")

    if args.cmd == "status":
        print(f"room: {room}")
        for m in bc.get("members") or []:
            pup = m.get("puppy") or ""
            tag = f"{m.get('user', '?')} ({pup})" if pup else m.get("user", "?")
            act = m.get("activity") or "idle"
            print(f"- {tag}: {act}")
        reply = rpc(relay, {"op": "list_claims", "room": room})
        items = reply.get("items") or []
        if items:
            print("active claims:")
            for c in items:
                print(f"- {describe_claim(c)}")
        return 0

    if args.cmd == "claims":
        reply = rpc(relay, {"op": "list_claims", "room": room})
        items = reply.get("items") or []
        if not items:
            print("no active claims")
        for c in items:
            print(describe_claim(c))
        return 0

    if args.cmd == "claim":
        reply = rpc(relay, {
            "op": "claim", "room": room, "user": user, "puppy": puppy,
            "path": args.path, "note": args.note,
        })
        if reply.get("ok"):
            print(f"claimed {args.path}")
            return 0
        holder = reply.get("holder")
        if holder:
            print(f"REFUSED: {describe_claim(holder)}")
            print("Pick different work or coordinate via `post` first.")
        else:
            print("REFUSED: room not active on the relay")
        return 1

    if args.cmd == "release":
        reply = rpc(relay, {"op": "release", "room": room, "user": user,
                            "path": args.path})
        if reply.get("ok"):
            print(f"released {args.path}")
            return 0
        print("REFUSED: you don't hold a claim on that path")
        return 1

    if args.cmd == "post":
        sender = f"{puppy} ({user}'s puppy)" if puppy else f"{user}'s puppy"
        reply = rpc(relay, {"op": "post", "room": room, "from": sender,
                            "text": args.message})
        if reply.get("ok"):
            print("posted to the pack")
            return 0
        print("REFUSED: room not active on the relay")
        return 1

    return 1


if __name__ == "__main__":
    sys.exit(main())
