#!/usr/bin/env python3
"""
Crawl an existing OtterSec-style verified-builds API and emit SQL seed
files for both the legacy schema (solana_program_builds + verified_programs
+ program_authority) and the v2 schema (builds + program_state).

Used to populate two databases with the same dataset so the new API's
responses can be diffed against the old one and benchmarked side by side.

Usage:
    scripts/seed_from_osec.py [--base-url URL] [--limit N] [--out-dir DIR]

Then:
    psql "$LEGACY_DATABASE_URL" -f seed_legacy.sql
    psql "$V2_DATABASE_URL"     -f seed_v2.sql

Notes:
- Public endpoints don't expose build params (lib_name, cargo_args,
  base_image, mount_path, arch, bpf_flag), so rows are seeded with NULL /
  defaults. Sufficient for /status, /status-all, /verified-programs,
  /verified-programs-status, /resolve-hash, /job, /logs comparison; not
  sufficient to drive a re-build.
- UUIDs are derived via uuid5 from (program_id, signer) so reruns are
  idempotent — `ON CONFLICT DO NOTHING` guards against accidental dupes.
- The script only uses the Python stdlib (no requests / psycopg2).
"""

import argparse
import json
import sys
import time
import uuid
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

DEFAULT_BASE = "https://verify.osec.io"
NAMESPACE = uuid.UUID("00000000-0000-0000-0000-000000000001")


def fetch(url):
    req = Request(url, headers={"Accept": "application/json"})
    with urlopen(req, timeout=30) as r:
        return json.loads(r.read())


def crawl_program_ids(base, max_pages=None):
    page = 1
    while True:
        if max_pages is not None and page > max_pages:
            return
        body = fetch(f"{base}/verified-programs/{page}")
        ids = body.get("verified_programs") or []
        if not ids:
            return
        for pid in ids:
            yield pid
        meta = body.get("meta") or {}
        if not meta.get("has_next_page"):
            return
        page += 1


def sql_lit(v):
    if v is None:
        return "NULL"
    if isinstance(v, bool):
        return "TRUE" if v else "FALSE"
    if isinstance(v, (int, float)):
        return str(v)
    escaped = str(v).replace("'", "''")
    return f"'{escaped}'"


def split_repo_url(repo_url, commit_fallback):
    """Reverse `build_repo_url`: <repo>/tree/<commit> -> (repo, commit)."""
    if not repo_url:
        return None, commit_fallback or None
    marker = "/tree/"
    if marker in repo_url:
        repo, _, commit = repo_url.partition(marker)
        return repo, commit_fallback or commit
    return repo_url, commit_fallback or None


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--base-url", default=DEFAULT_BASE)
    ap.add_argument(
        "--limit",
        type=int,
        default=0,
        help="cap number of /verified-programs pages crawled; 0 = all",
    )
    ap.add_argument("--out-dir", default=".")
    ap.add_argument(
        "--sleep",
        type=float,
        default=0.05,
        help="seconds to sleep between /status-all calls",
    )
    args = ap.parse_args()

    legacy_path = f"{args.out_dir}/seed_legacy.sql"
    v2_path = f"{args.out_dir}/seed_v2.sql"
    legacy = open(legacy_path, "w")
    v2 = open(v2_path, "w")

    for f in (legacy, v2):
        f.write("BEGIN;\n\n")

    seeded = 0
    skipped = 0
    page_limit = args.limit or None

    for pid in crawl_program_ids(args.base_url, page_limit):
        try:
            body = fetch(f"{args.base_url}/status-all/{pid}")
        except (HTTPError, URLError) as e:
            print(f"skip {pid}: {e}", file=sys.stderr)
            skipped += 1
            continue
        if not isinstance(body, list) or not body:
            skipped += 1
            continue

        first = body[0]
        is_frozen = bool(first.get("is_frozen"))
        is_closed = bool(first.get("is_closed"))
        on_chain_hash = first.get("on_chain_hash") or None

        legacy.write(
            "INSERT INTO program_authority "
            "(program_id, authority_id, last_updated, is_frozen, is_closed) "
            f"VALUES ({sql_lit(pid)}, NULL, NOW(), "
            f"{sql_lit(is_frozen)}, {sql_lit(is_closed)}) "
            "ON CONFLICT (program_id) DO NOTHING;\n"
        )
        v2.write(
            "INSERT INTO program_state "
            "(program_id, on_chain_hash, authority, is_frozen, is_closed, last_checked) "
            f"VALUES ({sql_lit(pid)}, {sql_lit(on_chain_hash)}, NULL, "
            f"{sql_lit(is_frozen)}, {sql_lit(is_closed)}, NOW()) "
            "ON CONFLICT (program_id) DO NOTHING;\n"
        )

        for entry in body:
            signer = entry.get("signer") or None
            exec_hash = entry.get("executable_hash") or None
            on_chain = entry.get("on_chain_hash") or ""
            repo, commit = split_repo_url(entry.get("repo_url"), entry.get("commit"))
            verified_at = entry.get("last_verified_at")
            ts = sql_lit(verified_at) if verified_at else "NOW()"

            build_id = str(uuid.uuid5(NAMESPACE, f"build|{pid}|{signer or ''}"))
            verified_id = str(uuid.uuid5(NAMESPACE, f"verified|{pid}|{signer or ''}"))

            legacy.write(
                "INSERT INTO solana_program_builds "
                "(id, repository, commit_hash, program_id, bpf_flag, created_at, status, signer) "
                f"VALUES ({sql_lit(build_id)}, {sql_lit(repo or '')}, {sql_lit(commit)}, "
                f"{sql_lit(pid)}, FALSE, {ts}, 'completed', {sql_lit(signer)}) "
                "ON CONFLICT (id) DO NOTHING;\n"
            )
            legacy.write(
                "INSERT INTO verified_programs "
                "(id, program_id, is_verified, on_chain_hash, executable_hash, "
                "verified_at, solana_build_id) "
                f"VALUES ({sql_lit(verified_id)}, {sql_lit(pid)}, "
                f"{sql_lit(bool(entry.get('is_verified')))}, {sql_lit(on_chain)}, "
                f"{sql_lit(exec_hash or '')}, {ts}, {sql_lit(build_id)}) "
                "ON CONFLICT (id) DO NOTHING;\n"
            )

            v2.write(
                "INSERT INTO builds "
                "(id, repository, commit_hash, program_id, bpf_flag, status, "
                "executable_hash, signer, created_at, completed_at) "
                f"VALUES ({sql_lit(build_id)}, {sql_lit(repo or '')}, {sql_lit(commit)}, "
                f"{sql_lit(pid)}, FALSE, 'completed', {sql_lit(exec_hash)}, "
                f"{sql_lit(signer)}, {ts}, {ts}) "
                "ON CONFLICT (id) DO NOTHING;\n"
            )

        seeded += 1
        if seeded % 25 == 0:
            print(f"seeded {seeded} programs (skipped {skipped})", file=sys.stderr)
        time.sleep(args.sleep)

    for f in (legacy, v2):
        f.write("\nCOMMIT;\n")
        f.close()

    print(
        f"done: {seeded} programs, {skipped} skipped\n"
        f"  legacy -> {legacy_path}\n"
        f"  v2     -> {v2_path}",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
