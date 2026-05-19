//! Verification-related DB writes.
//!
//! In the content-addressed model there is no "is this program verified?"
//! state to maintain — `/status` answers it live by looking the current
//! on-chain hash up in `verified_hashes`. The only persistent write needed
//! around a build is updating the `solana_program_builds` job row when the
//! build completes (job-status tracking), plus the directory write itself
//! (handled in `db::hashes`).
