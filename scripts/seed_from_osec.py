#!/usr/bin/env python3
"""
Crawl an existing OtterSec-style verified-builds API and emit SQL seed
files for both the legacy schema (solana_program_builds + verified_programs
+ program_authority) and the v2 schema (builds + program_state).

Used to populate two databases with the same dataset so the new API's
responses can be diffed against the old one and benchmarked side by side.

Usage:
    scripts/seed_from_osec.py [--base-url URL] [--rpc-url URL]
                              [--limit N] [--out-dir DIR]

Then:
    psql "$LEGACY_DATABASE_URL" -f seed_legacy.sql
    psql "$V2_DATABASE_URL"     -f seed_v2.sql

When --rpc-url is given, the script enriches build rows with on-chain
Otter Verify PDA params (lib_name, bpf_flag, base_image, mount_path,
cargo_args, arch) — fetched via a single `getProgramAccounts` call against
the Otter Verify program. The public API responses don't include these
fields, so without --rpc-url those columns default to NULL/FALSE.

The RPC endpoint must permit `getProgramAccounts`. Most free public RPCs
do not — use Helius / Triton / your own validator.

UUIDs are derived via uuid5 from (program_id, signer) so reruns are
idempotent; `ON CONFLICT DO NOTHING` guards against re-inserting on top
of existing data.
"""

import argparse
import base64
import json
import sys
import time
import uuid
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

DEFAULT_BASE = "https://verify.osec.io"
OTTER_VERIFY_PROGRAM = "verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC"
NAMESPACE = uuid.UUID("00000000-0000-0000-0000-000000000001")

BASE58 = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58_encode(b: bytes) -> str:
    n = int.from_bytes(b, "big")
    out = bytearray()
    while n > 0:
        n, rem = divmod(n, 58)
        out.append(BASE58[rem])
    for byte in b:
        if byte == 0:
            out.append(BASE58[0])
        else:
            break
    return bytes(reversed(out)).decode("ascii")


def parse_otter_pda(raw: bytes) -> dict:
    """Borsh-deserialize an Otter Verify PDA (after the 8-byte Anchor disc)."""
    p = [8]

    def take(n):
        v = raw[p[0] : p[0] + n]
        p[0] += n
        return v

    def take_string():
        n = int.from_bytes(take(4), "little")
        return take(n).decode("utf-8", "replace")

    def take_vec_string():
        n = int.from_bytes(take(4), "little")
        return [take_string() for _ in range(n)]

    address = b58_encode(take(32))
    signer = b58_encode(take(32))
    _version = take_string()
    git_url = take_string()
    commit = take_string()
    args = take_vec_string()
    _deployed_slot = int.from_bytes(take(8), "little")
    _bump = take(1)
    return {
        "address": address,
        "signer": signer,
        "git_url": git_url,
        "commit": commit,
        "args": args,
    }


def derive_build_fields(args: list) -> dict:
    """Mirror of `OtterBuildParams::{bpf, library_name, base_image, ...}`."""

    def after(flags):
        for i, a in enumerate(args):
            if a in flags and i + 1 < len(args):
                return args[i + 1]
        return None

    bpf = "--bpf" in args
    lib_name = after(("--library-name",))
    base_image = after(("--base-image", "-b"))
    mount_path = after(("--mount-path",))
    arch = after(("--arch",))
    cargo_args = None
    if "--" in args:
        cargo_args = args[args.index("--") + 1 :]
    return {
        "bpf_flag": bpf,
        "lib_name": lib_name,
        "base_image": base_image,
        "mount_path": mount_path,
        "arch": arch,
        "cargo_args": cargo_args,
    }


def fetch_json(url, **kw):
    req = Request(url, headers={"Accept": "application/json"})
    with urlopen(req, timeout=30, **kw) as r:
        return json.loads(r.read())


def rpc(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = Request(url, data=body, headers={"content-type": "application/json"})
    with urlopen(req, timeout=120) as r:
        resp = json.loads(r.read())
    if "error" in resp:
        raise RuntimeError(f"RPC {method} error: {resp['error']}")
    return resp["result"]


def fetch_otter_pdas(rpc_url):
    """Return dict keyed by (signer, program_id) -> parsed PDA fields."""
    accts = rpc(rpc_url, "getProgramAccounts", [OTTER_VERIFY_PROGRAM, {"encoding": "base64"}])
    out = {}
    for a in accts:
        try:
            raw = base64.b64decode(a["account"]["data"][0])
            parsed = parse_otter_pda(raw)
        except Exception as e:
            print(f"skip PDA {a.get('pubkey')}: {e}", file=sys.stderr)
            continue
        key = (parsed["signer"], parsed["address"])
        out[key] = parsed
    return out


def crawl_program_ids(base, max_pages=None):
    page = 1
    while True:
        if max_pages is not None and page > max_pages:
            return
        body = fetch_json(f"{base}/verified-programs/{page}")
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
    return "'" + str(v).replace("'", "''") + "'"


def sql_text_array(v):
    if v is None:
        return "NULL"
    inner = ",".join('"' + str(x).replace("\\", "\\\\").replace('"', '\\"') + '"' for x in v)
    return "'{" + inner.replace("'", "''") + "}'"


def split_repo_url(repo_url, commit_fallback):
    if not repo_url:
        return None, commit_fallback or None
    if "/tree/" in repo_url:
        repo, _, commit = repo_url.partition("/tree/")
        return repo, commit_fallback or commit
    return repo_url, commit_fallback or None


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--base-url", default=DEFAULT_BASE)
    ap.add_argument(
        "--rpc-url",
        help="Solana RPC URL with getProgramAccounts enabled; enriches build rows with PDA params",
    )
    ap.add_argument("--limit", type=int, default=0, help="cap pages crawled; 0 = all")
    ap.add_argument("--out-dir", default=".")
    ap.add_argument("--sleep", type=float, default=0.05)
    args = ap.parse_args()

    pdas = {}
    if args.rpc_url:
        print(f"fetching Otter Verify PDAs from {args.rpc_url} ...", file=sys.stderr)
        pdas = fetch_otter_pdas(args.rpc_url)
        print(f"  parsed {len(pdas)} PDAs", file=sys.stderr)

    legacy_path = f"{args.out_dir}/seed_legacy.sql"
    v2_path = f"{args.out_dir}/seed_v2.sql"
    legacy = open(legacy_path, "w")
    v2 = open(v2_path, "w")
    for f in (legacy, v2):
        f.write("BEGIN;\n\n")

    seeded = skipped = 0
    page_limit = args.limit or None

    for pid in crawl_program_ids(args.base_url, page_limit):
        try:
            body = fetch_json(f"{args.base_url}/status-all/{pid}")
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
            "(program_id, authority_id, last_updated, is_frozen, is_closed) VALUES "
            f"({sql_lit(pid)}, NULL, NOW(), {sql_lit(is_frozen)}, {sql_lit(is_closed)}) "
            "ON CONFLICT (program_id) DO NOTHING;\n"
        )
        v2.write(
            "INSERT INTO program_state "
            "(program_id, on_chain_hash, authority, is_frozen, is_closed, last_checked) VALUES "
            f"({sql_lit(pid)}, {sql_lit(on_chain_hash)}, NULL, "
            f"{sql_lit(is_frozen)}, {sql_lit(is_closed)}, NOW()) "
            "ON CONFLICT (program_id) DO NOTHING;\n"
        )

        for entry in body:
            signer = entry.get("signer") or None
            exec_hash = entry.get("executable_hash") or None
            on_chain = entry.get("on_chain_hash") or ""
            api_repo, api_commit = split_repo_url(entry.get("repo_url"), entry.get("commit"))
            verified_at = entry.get("last_verified_at")
            ts = sql_lit(verified_at) if verified_at else "NOW()"

            # Default build params (when --rpc-url isn't given or PDA isn't found)
            repo = api_repo or ""
            commit = api_commit if api_commit and api_commit != "None" else None
            fields = {
                "bpf_flag": False,
                "lib_name": None,
                "base_image": None,
                "mount_path": None,
                "arch": None,
                "cargo_args": None,
            }
            if signer and (signer, pid) in pdas:
                pda = pdas[(signer, pid)]
                repo = pda["git_url"] or repo
                commit = pda["commit"] or commit
                fields = derive_build_fields(pda["args"])

            build_id = str(uuid.uuid5(NAMESPACE, f"build|{pid}|{signer or ''}"))
            verified_id = str(uuid.uuid5(NAMESPACE, f"verified|{pid}|{signer or ''}"))

            legacy.write(
                "INSERT INTO solana_program_builds "
                "(id, repository, commit_hash, program_id, lib_name, base_docker_image, "
                "mount_path, cargo_args, bpf_flag, created_at, status, signer, arch) VALUES "
                f"({sql_lit(build_id)}, {sql_lit(repo)}, {sql_lit(commit)}, {sql_lit(pid)}, "
                f"{sql_lit(fields['lib_name'])}, {sql_lit(fields['base_image'])}, "
                f"{sql_lit(fields['mount_path'])}, {sql_text_array(fields['cargo_args'])}, "
                f"{sql_lit(fields['bpf_flag'])}, {ts}, 'completed', "
                f"{sql_lit(signer)}, {sql_lit(fields['arch'])}) "
                "ON CONFLICT (id) DO NOTHING;\n"
            )
            legacy.write(
                "INSERT INTO verified_programs "
                "(id, program_id, is_verified, on_chain_hash, executable_hash, "
                "verified_at, solana_build_id) VALUES "
                f"({sql_lit(verified_id)}, {sql_lit(pid)}, "
                f"{sql_lit(bool(entry.get('is_verified')))}, {sql_lit(on_chain)}, "
                f"{sql_lit(exec_hash or '')}, {ts}, {sql_lit(build_id)}) "
                "ON CONFLICT (id) DO NOTHING;\n"
            )

            v2.write(
                "INSERT INTO builds "
                "(id, repository, commit_hash, program_id, lib_name, base_docker_image, "
                "mount_path, cargo_args, bpf_flag, arch, signer, status, "
                "executable_hash, created_at, completed_at) VALUES "
                f"({sql_lit(build_id)}, {sql_lit(repo)}, {sql_lit(commit)}, {sql_lit(pid)}, "
                f"{sql_lit(fields['lib_name'])}, {sql_lit(fields['base_image'])}, "
                f"{sql_lit(fields['mount_path'])}, {sql_text_array(fields['cargo_args'])}, "
                f"{sql_lit(fields['bpf_flag'])}, {sql_lit(fields['arch'])}, {sql_lit(signer)}, "
                f"'completed', {sql_lit(exec_hash)}, {ts}, {ts}) "
                "ON CONFLICT (id) DO NOTHING;\n"
            )

        seeded += 1
        if seeded % 25 == 0:
            print(f"seeded {seeded} programs (skipped {skipped})", file=sys.stderr)
        time.sleep(args.sleep)

    for f in (legacy, v2):
        f.write("\nCOMMIT;\n")
        f.close()

    enriched = "with PDA enrichment" if args.rpc_url else "without PDA enrichment (use --rpc-url)"
    print(
        f"done: {seeded} programs, {skipped} skipped ({enriched})\n"
        f"  legacy -> {legacy_path}\n"
        f"  v2     -> {v2_path}",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
