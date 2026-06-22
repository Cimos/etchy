#!/usr/bin/env python3
"""Merge + triage all demo feedback in deploy/feedback/*.jsonl.

Reads every *.jsonl in deploy/feedback/ (one submission per line), de-dupes,
and prints a summary (counts by sentiment / category / severity) followed by the
records sorted by time. Stdlib only.

Usage:
  python3 deploy/collect-feedback.py            # summary + records to stdout
  python3 deploy/collect-feedback.py --merged   # also emit merged JSONL (one obj/line)
"""
import collections
import glob
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
FEEDBACK_DIR = os.path.join(HERE, "feedback")


def load():
    recs, seen = [], set()
    for path in sorted(glob.glob(os.path.join(FEEDBACK_DIR, "*.jsonl"))):
        src = os.path.basename(path)
        with open(path, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    r = json.loads(line)
                except json.JSONDecodeError:
                    continue
                key = (r.get("ts"), r.get("ip"), r.get("text") or r.get("message"))
                if key in seen:
                    continue
                seen.add(key)
                r["_src"] = src
                recs.append(r)
    recs.sort(key=lambda r: r.get("ts", ""))
    return recs


def main():
    if not os.path.isdir(FEEDBACK_DIR):
        print(f"no feedback dir: {FEEDBACK_DIR}")
        return 1
    recs = load()
    if not recs:
        print("no feedback records found in deploy/feedback/*.jsonl")
        return 0

    if "--merged" in sys.argv:
        for r in recs:
            r.pop("_src", None)
            print(json.dumps(r, ensure_ascii=False))
        return 0

    def tally(field):
        c = collections.Counter(r.get(field, "—") or "—" for r in recs)
        return ", ".join(f"{k}: {n}" for k, n in c.most_common())

    files = sorted({r["_src"] for r in recs})
    print(f"== etchy feedback — {len(recs)} records across {len(files)} file(s): {', '.join(files)} ==\n")
    print(f"sentiment : {tally('sentiment')}")
    print(f"category  : {tally('category')}")
    print(f"severity  : {tally('severity')}\n")
    print("-" * 72)
    for r in recs:
        who = r.get("name") or "anon"
        sev = r.get("severity")
        head = f"[{r.get('sentiment','?')}] {r.get('category','?')}"
        if sev:
            head += f" / {sev}"
        app = (r.get("ctx") or {}).get("app") if isinstance(r.get("ctx"), dict) else None
        ctx = f"  ({app})" if app else ""
        print(f"\n{r.get('ts','')}  {who}  {head}{ctx}")
        print(f"  {r.get('text') or r.get('message') or ''}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
