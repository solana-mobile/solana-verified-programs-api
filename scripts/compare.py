#!/usr/bin/env python3
"""
Fire the same GET requests at two API instances and report response diffs
plus per-endpoint latency stats. Use to validate the rewrite returns the
same payloads as the legacy API, and to spot performance regressions.

Usage:
    scripts/compare.py --legacy http://127.0.0.1:3000 --v2 http://127.0.0.1:3001 \
        [--programs FILE | --sample N] [--repeats N]

`--programs FILE` is one program ID per line. With `--sample N` the script
crawls /verified-programs/1 from the legacy API and picks the first N IDs.

Latency reports the median, p95, and max across `--repeats` measurements
per endpoint per API.
"""

import argparse
import json
import statistics
import sys
import time
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


def get(url, timeout=10):
    """Return (status, body_text, elapsed_seconds). Status is `None` on timeout."""
    req = Request(url, headers={"Accept": "application/json"})
    t0 = time.monotonic()
    try:
        with urlopen(req, timeout=timeout) as r:
            body = r.read().decode("utf-8", "replace")
            status = r.status
    except HTTPError as e:
        body = e.read().decode("utf-8", "replace") if e.fp else ""
        status = e.code
    except (URLError, TimeoutError) as e:
        body = f"{e}"
        status = None
    elapsed = time.monotonic() - t0
    return status, body, elapsed


def normalise(j):
    """Drop fields that legitimately differ between instances (timestamps,
    background-job state, sweep age, etc.) before diffing."""
    if isinstance(j, dict):
        return {
            k: normalise(v)
            for k, v in j.items()
            if k not in {"timestamp", "last_program_check", "background_jobs", "sweep"}
        }
    if isinstance(j, list):
        return [normalise(x) for x in j]
    return j


def diff(a_body, b_body):
    try:
        a = json.loads(a_body)
        b = json.loads(b_body)
    except json.JSONDecodeError:
        return None if a_body == b_body else "non-JSON, raw differs"
    if normalise(a) == normalise(b):
        return None
    # produce a compact diff
    import difflib

    pa = json.dumps(normalise(a), indent=2, sort_keys=True).splitlines()
    pb = json.dumps(normalise(b), indent=2, sort_keys=True).splitlines()
    return "\n".join(difflib.unified_diff(pa, pb, "legacy", "v2", lineterm=""))


def percentile(xs, p):
    if not xs:
        return 0.0
    xs = sorted(xs)
    k = (len(xs) - 1) * p
    f = int(k)
    c = min(f + 1, len(xs) - 1)
    return xs[f] + (xs[c] - xs[f]) * (k - f)


def fmt_ms(s):
    return f"{s * 1000:.1f}ms"


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--legacy", required=True)
    ap.add_argument("--v2", required=True)
    ap.add_argument("--programs", help="file with one program ID per line")
    ap.add_argument("--sample", type=int, default=10, help="programs to pick from legacy if no --programs")
    ap.add_argument("--repeats", type=int, default=20, help="latency samples per endpoint")
    ap.add_argument("--no-bench", action="store_true", help="skip latency runs")
    ap.add_argument(
        "--slow",
        action="store_true",
        help="also benchmark /verified-programs-status (legacy RPCs per program, slow)",
    )
    ap.add_argument("--timeout", type=float, default=10.0)
    args = ap.parse_args()

    if args.programs:
        with open(args.programs) as f:
            programs = [ln.strip() for ln in f if ln.strip()]
    else:
        _, body, _ = get(f"{args.legacy}/verified-programs/1")
        programs = (json.loads(body).get("verified_programs") or [])[: args.sample]

    print(f"comparing {len(programs)} program(s)\n")

    # /verified-programs-status omitted by default — legacy RPCs per program
    # and routinely takes tens of seconds with hundreds of rows.
    endpoints = [("/verified-programs/1", [])]
    if args.slow:
        endpoints.append(("/verified-programs-status", []))
    for pid in programs:
        endpoints.append((f"/status/{pid}", [pid]))
        endpoints.append((f"/status-all/{pid}", [pid]))

    # ---- correctness pass ----
    mismatches = 0
    for path, _ in endpoints:
        sa, ba, _ = get(args.legacy + path)
        sb, bb, _ = get(args.v2 + path)
        if sa != sb:
            print(f"[STATUS MISMATCH] {path}: legacy={sa} v2={sb}")
            mismatches += 1
            continue
        d = diff(ba, bb)
        if d:
            mismatches += 1
            print(f"[BODY DIFF] {path}")
            print(d[:2000])
            print()
        else:
            print(f"[OK]   {path}")
    print(f"\n{mismatches} mismatches out of {len(endpoints)} endpoints\n")

    if args.no_bench:
        return

    # ---- latency pass ----
    print(f"latency: {args.repeats} samples per endpoint per side\n")
    print(f"{'endpoint':<60} {'legacy med/p95':>22} {'v2 med/p95':>22} {'speedup':>10}")
    for path, _ in endpoints:
        la, lv = [], []
        for _ in range(args.repeats):
            _, _, e = get(args.legacy + path, timeout=args.timeout)
            la.append(e)
            _, _, e = get(args.v2 + path, timeout=args.timeout)
            lv.append(e)
        la_med, la_p95 = statistics.median(la), percentile(la, 0.95)
        lv_med, lv_p95 = statistics.median(lv), percentile(lv, 0.95)
        speedup = (la_med / lv_med) if lv_med > 0 else float("inf")
        print(
            f"{path[:60]:<60} "
            f"{fmt_ms(la_med):>10}/{fmt_ms(la_p95):>10} "
            f"{fmt_ms(lv_med):>10}/{fmt_ms(lv_p95):>10} "
            f"{speedup:>9.2f}x"
        )


if __name__ == "__main__":
    main()
