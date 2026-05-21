# Proposal: Content-Addressed Verified Builds API

Implemented in API PR #1 and CLI PR #7.

The verified-builds API is a directory of verified **build claims**, keyed by
`(executable_hash, signer)`, mapping to `(repository, commit, build_args)`.
A row asserts that signer `signer` claims those bytes were produced by that
build config. Multiple signers may claim the same hash; each is its own row.
No `is_verified` flag, no invalidation, no staleness. Consumers hash whatever
bytes they have (deployed program, buffer, local `.so`) and query the same
way, so the API only ever attests to the (hash ↔ build, attributed-to-signer)
mapping — the half it actually has authority over. Upgrades self-resolve:
the new on-chain hash either matches a directory row from a trusted signer
or it doesn't. The PDA stops doing double duty and becomes purely the
deployer's on-chain claim of association.

Removed: `/unverify`, `/pda`, `/verified-programs*`, the `verified_programs`
table, the reverify cycle, the background refresh job. ~3000 lines.

## Endpoints

- **`GET /status/:program_id`** — trust-filtered single answer: fetch the
  on-chain hash, return verified=true iff a directory row exists with that
  hash and a signer in `{ program upgrade authority } ∪ SIGNER_KEYS` (the
  whitelisted Otter signers). Response surfaces which signer satisfied the
  filter.
- **`GET /status-all/:id`** — polymorphic list lookup.
  - If `:id` is a 64-char hex hash: every signer's claim for that hash
    (caller decides whose claim to trust).
  - If `:id` is a base58 pubkey: every trusted signer's claim for the
    program's current on-chain hash.
- **`POST /verify`, `/verify-with-signer`** — submit a build config. Cache
  hit on `(repo, commit, build_args)` returns the hash immediately and
  records this signer's claim about it; cache miss queues a build whose
  completion writes the directory row.

## Why

- **No staleness.** Upgrades either match a known hash or don't. Always live.
- **Buffer verification falls out for free.** Same primitive (look up the
  hash through `/status-all`).
- **Stronger trust model.** Consumer hashes the bytes themselves; the API
  attributes the (hash ↔ build) claim to a specific signer; `/status`
  refuses to count claims from outside the trust set.
- **PDA stops doing double duty.** Just the deployer's on-chain claim,
  not also the build trigger.

## Open Questions

- Drop the PDA gate on `/verify*` entirely? Needs a DoS replacement first.
- Restructure the PDA's `args: Vec<String>` argv to a typed `BuildArgs`;
  drop the dead `address`, `version`, `deployed_slot` fields.
- Slim `/verify`'s request body to its load-bearing fields (`program_id`,
  `webhook_url`); fold `/verify` and `/verify-with-signer` into one
  endpoint with `signer: Option<String>`.
- Retire `/verify_sync` (back-end for the deprecated `--remote` CLI flag).
