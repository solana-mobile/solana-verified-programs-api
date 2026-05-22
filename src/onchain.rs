//! Everything that touches the Solana chain.

use crate::{error::ApiError, error::Result, rpc::get_rpc_manager};
use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use solana_account_decoder::parse_bpf_loader::{
    parse_bpf_upgradeable_loader, BpfUpgradeableLoaderAccountType, UiProgram, UiProgramData,
};
use solana_client::{
    nonblocking::rpc_client::RpcClient, rpc_client::GetConfirmedSignaturesForAddress2Config,
    rpc_config::RpcTransactionConfig,
};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_sdk_ids::{bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable};
use solana_transaction_status::{EncodedTransaction, UiMessage, UiTransactionEncoding};
use std::{collections::HashMap, str::FromStr};
use tracing::warn;

pub const OTTER_VERIFY_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC");

/// Whitelisted Otter Verify signers, tried in order when no explicit signer
/// or program-authority claim exists.
pub const SIGNER_KEYS: [Pubkey; 3] = [
    solana_pubkey::pubkey!("9VWiUUhgNoRwTH5NVehYJEDwcotwYX3VgW4MChiHPAqU"),
    solana_pubkey::pubkey!("CyJj5ejJAUveDXnLduJbkvwjxcmWJNqCuB9DR7AExrHn"),
    solana_pubkey::pubkey!("5vJwnLeyjV8uNJSp1zn7VLW8GwiQbcsQbGaVSwRmkE4r"),
];

// Stable per the BPF loader v3 spec:
// bincode(UpgradeableLoaderState::ProgramData{slot, Some(Pubkey)}) =
//   4-byte enum tag + 8-byte slot + 1-byte option discriminator + 32-byte pubkey.
const PROGRAM_DATA_HEADER_SIZE: usize = 45;

// Solana RPC servers cap getMultipleAccounts at 100.
const GMA_CHUNK: usize = 100;

// Squads-frozen programs route the final upgrade through the Squads multisig;
// the burned authority is the 5th account on this specific instruction.
const SQUADS_PROGRAM_ID: &str = "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf";
const SQUADS_AUTHORITY_IX_DATA: &str = "ZTNTtVtnvbC";
const SQUADS_AUTHORITY_ACCOUNT_INDEX: usize = 4;

/// Snapshot of the on-chain side of a program — what gets written to `program_state`.
#[derive(Debug, Clone)]
pub struct ProgramOnchainState {
    pub authority: Option<String>,
    pub is_frozen: bool,
    pub is_closed: bool,
    pub executable_hash: Option<String>,
}

impl ProgramOnchainState {
    fn closed() -> Self {
        Self {
            authority: None,
            is_frozen: false,
            is_closed: true,
            executable_hash: None,
        }
    }

    fn empty() -> Self {
        Self {
            authority: None,
            is_frozen: false,
            is_closed: false,
            executable_hash: None,
        }
    }
}

/// Borsh layout of an Otter Verify PDA. The leading 8-byte Anchor
/// discriminator is stripped before calling `try_from_slice`.
#[derive(BorshDeserialize, BorshSerialize, Debug)]
pub struct OtterBuildParams {
    pub address: Pubkey,
    pub signer: Pubkey,
    pub version: String,
    pub git_url: String,
    pub commit: String,
    /// Raw `solana-verify verify-from-repo` argv. Accessor methods below
    /// decode the flags we care about.
    pub args: Vec<String>,
    pub deployed_slot: u64,
    bump: u8,
}

impl OtterBuildParams {
    /// Whether the claim asks for `cargo build-bpf` instead of `build-sbf`.
    pub fn bpf(&self) -> bool {
        self.args.iter().any(|a| a == "--bpf")
    }

    fn arg_after(&self, names: &[&str]) -> Option<String> {
        let pos = self.args.iter().position(|a| names.contains(&a.as_str()))?;
        self.args.get(pos + 1).cloned()
    }

    pub fn mount_path(&self) -> Option<String> {
        self.arg_after(&["--mount-path"])
    }

    pub fn library_name(&self) -> Option<String> {
        self.arg_after(&["--library-name"])
    }

    pub fn base_image(&self) -> Option<String> {
        self.arg_after(&["--base-image", "-b"])
    }

    pub fn arch(&self) -> Option<String> {
        self.arg_after(&["--arch"])
    }

    /// Everything after the first `--`.
    pub fn cargo_args(&self) -> Option<Vec<String>> {
        let pos = self.args.iter().position(|a| a == "--")?;
        Some(self.args[pos + 1..].to_vec())
    }
}

/// Batched on-chain snapshot for many programs at once.
///
/// Two `getMultipleAccounts` calls per chunk of 100: one for the program
/// accounts (to extract program-data PDAs and handle legacy loaders), one
/// for the program-data accounts. Executable hash is computed inline from
/// the bytes — no `solana-verify` subprocess.
///
/// For programs frozen with no authority on the program-data account,
/// `authority` is left `None` and `is_frozen` is `true`. The Squads
/// transaction-history recovery only runs from [`get_program_state`].
pub async fn snapshot_programs(ids: &[Pubkey]) -> Result<HashMap<Pubkey, ProgramOnchainState>> {
    let mut out = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(GMA_CHUNK) {
        snapshot_chunk(chunk, &mut out).await?;
    }
    Ok(out)
}

async fn snapshot_chunk(
    ids: &[Pubkey],
    out: &mut HashMap<Pubkey, ProgramOnchainState>,
) -> Result<()> {
    let ids_vec = ids.to_vec();
    let accounts = get_rpc_manager()
        .execute_with_retry(|client| {
            let ids_vec = ids_vec.clone();
            async move {
                client
                    .get_multiple_accounts(&ids_vec)
                    .await
                    .map_err(|e| ApiError::Rpc(format!("getMultipleAccounts: {e}")))
            }
        })
        .await?;

    let mut to_fetch: Vec<(Pubkey, Pubkey)> = Vec::new();
    for (id, maybe_acc) in ids.iter().zip(accounts.into_iter()) {
        let Some(acc) = maybe_acc else {
            out.insert(*id, ProgramOnchainState::closed());
            continue;
        };
        if acc.owner == bpf_loader_upgradeable::ID {
            match extract_program_data_pda(&acc.data) {
                Ok(pda) => to_fetch.push((*id, pda)),
                Err(e) => {
                    warn!("program {} unparseable: {}", id, e);
                    out.insert(*id, ProgramOnchainState::closed());
                }
            }
        } else if acc.owner == bpf_loader::ID || acc.owner == bpf_loader_deprecated::ID {
            // Legacy loaders: the account data IS the executable; immutable.
            out.insert(
                *id,
                ProgramOnchainState {
                    authority: None,
                    is_frozen: true,
                    is_closed: false,
                    executable_hash: Some(compute_program_hash(&acc.data)),
                },
            );
        } else {
            warn!("program {} has unsupported owner {}", id, acc.owner);
            out.insert(*id, ProgramOnchainState::closed());
        }
    }

    if to_fetch.is_empty() {
        return Ok(());
    }

    let pdas: Vec<Pubkey> = to_fetch.iter().map(|(_, p)| *p).collect();
    let pda_accounts = get_rpc_manager()
        .execute_with_retry(|client| {
            let pdas = pdas.clone();
            async move {
                client
                    .get_multiple_accounts(&pdas)
                    .await
                    .map_err(|e| ApiError::Rpc(format!("getMultipleAccounts(program_data): {e}")))
            }
        })
        .await?;

    for ((program_id, _), maybe_acc) in to_fetch.iter().zip(pda_accounts.into_iter()) {
        match maybe_acc {
            None => {
                out.insert(*program_id, ProgramOnchainState::closed());
            }
            Some(acc) => {
                let authority = parse_program_data_authority(&acc.data);
                let hash = if acc.data.len() > PROGRAM_DATA_HEADER_SIZE {
                    Some(compute_program_hash(&acc.data[PROGRAM_DATA_HEADER_SIZE..]))
                } else {
                    None
                };
                out.insert(
                    *program_id,
                    ProgramOnchainState {
                        is_frozen: authority.is_none(),
                        authority,
                        is_closed: false,
                        executable_hash: hash,
                    },
                );
            }
        }
    }
    Ok(())
}

fn extract_program_data_pda(data: &[u8]) -> Result<Pubkey> {
    match parse_bpf_upgradeable_loader(data)? {
        BpfUpgradeableLoaderAccountType::Program(UiProgram { program_data }) => {
            Pubkey::from_str(&program_data).map_err(Into::into)
        }
        other => Err(ApiError::Rpc(format!(
            "expected Program account, got: {other:?}"
        ))),
    }
}

fn parse_program_data_authority(data: &[u8]) -> Option<String> {
    match parse_bpf_upgradeable_loader(data) {
        Ok(BpfUpgradeableLoaderAccountType::ProgramData(UiProgramData { authority, .. })) => {
            authority
        }
        _ => None,
    }
}

/// `sha256(data with trailing zeros stripped)`, hex-encoded. Matches
/// `solana-verify get-program-hash`'s output byte-for-byte.
//
// TODO: solana-verify is binary-only today. If it ships a library crate
// upstream we can drop this function and the PROGRAM_DATA_HEADER_SIZE
// constant in favour of their `get_binary_hash` /
// `UpgradeableLoaderState::size_of_programdata_metadata()`.
fn compute_program_hash(data: &[u8]) -> String {
    let trimmed = match data.iter().rposition(|&b| b != 0) {
        Some(i) => &data[..=i],
        None => &[][..],
    };
    let mut hasher = Sha256::new();
    hasher.update(trimmed);
    hex::encode(hasher.finalize())
}

/// Single-program snapshot, with Squads/burned-authority recovery via tx
/// Convenience wrapper around `get_program_state` that returns just the
/// on-chain executable hash as a hex string.
pub async fn get_on_chain_hash(program_id: &str) -> Result<String> {
    let pid = Pubkey::from_str(program_id).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let state = get_program_state(&pid).await?;
    Ok(state.executable_hash.unwrap_or_default())
}

/// history when the program looks frozen but has no on-chain authority.
/// Used by the verify path where the authority drives Otter Verify PDA lookup.
pub async fn get_program_state(program_id: &Pubkey) -> Result<ProgramOnchainState> {
    let mut state = snapshot_programs(&[*program_id])
        .await?
        .remove(program_id)
        .unwrap_or_else(ProgramOnchainState::empty);
    if state.is_frozen && state.authority.is_none() && !state.is_closed {
        if let Ok(Some(auth)) = recover_burned_authority(program_id).await {
            state.authority = Some(auth);
        }
    }
    Ok(state)
}

async fn recover_burned_authority(program_id: &Pubkey) -> Result<Option<String>> {
    let program_data_pda =
        Pubkey::find_program_address(&[program_id.as_ref()], &bpf_loader_upgradeable::id()).0;
    get_rpc_manager()
        .execute_with_retry(|client| async move {
            let cfg = GetConfirmedSignaturesForAddress2Config {
                limit: Some(1),
                before: None,
                until: None,
                commitment: None,
            };
            let sigs = client
                .get_signatures_for_address_with_config(&program_data_pda, cfg)
                .await
                .map_err(|e| ApiError::Rpc(e.to_string()))?;
            let Some(latest) = sigs.first() else {
                return Ok(None);
            };
            let sig = Signature::from_str(&latest.signature)
                .map_err(|e| ApiError::Rpc(format!("parse signature: {e}")))?;
            let tx = client
                .get_transaction_with_config(
                    &sig,
                    RpcTransactionConfig {
                        encoding: Some(UiTransactionEncoding::Json),
                        commitment: None,
                        max_supported_transaction_version: Some(0),
                    },
                )
                .await?;
            if let EncodedTransaction::Json(ui) = tx.transaction.transaction {
                if let UiMessage::Raw(raw) = &ui.message {
                    if let Some(squads_idx) =
                        raw.account_keys.iter().position(|k| k == SQUADS_PROGRAM_ID)
                    {
                        let squads_idx = squads_idx as u8;
                        for ix in &raw.instructions {
                            if ix.program_id_index == squads_idx
                                && ix.data == SQUADS_AUTHORITY_IX_DATA
                            {
                                let aidx = ix.accounts[SQUADS_AUTHORITY_ACCOUNT_INDEX] as usize;
                                return Ok(Some(raw.account_keys[aidx].clone()));
                            }
                        }
                    }
                    return Ok(Some(raw.account_keys[0].clone()));
                }
            }
            Ok(None)
        })
        .await
}

/// Seeds: `("otter_verify", signer, program)`.
pub async fn get_otter_pda(
    client: &RpcClient,
    signer: &Pubkey,
    program: &Pubkey,
) -> Result<OtterBuildParams> {
    let seeds: &[&[u8]] = &[b"otter_verify", &signer.to_bytes(), &program.to_bytes()];
    let (pda, _) = Pubkey::find_program_address(seeds, &OTTER_VERIFY_PROGRAM_ID);
    let data = client.get_account_data(&pda).await?;
    OtterBuildParams::try_from_slice(&data[8..])
        .map_err(|e| ApiError::Internal(format!("deserialize PDA: {e}")))
}

/// Resolution order: `explicit_signer`, then the program's current upgrade
/// `authority`, then [`SIGNER_KEYS`] in order. `NotFound` if none has a PDA.
pub async fn get_otter_verify_params(
    program: &Pubkey,
    explicit_signer: Option<Pubkey>,
    authority: Option<&str>,
) -> Result<(OtterBuildParams, String)> {
    let authority = authority.map(|s| s.to_string());
    let program = *program;

    get_rpc_manager()
        .execute_with_retry(move |client| {
            let authority = authority.clone();
            async move {
                if let Some(signer) = explicit_signer {
                    return match get_otter_pda(&client, &signer, &program).await {
                        Ok(p) => Ok((p, signer.to_string())),
                        Err(_) => Err(ApiError::NotFound(format!(
                            "Otter-Verify PDA not found for signer: {signer}"
                        ))),
                    };
                }
                if let Some(authority_str) = authority {
                    if let Ok(auth) = Pubkey::from_str(&authority_str) {
                        if let Ok(p) = get_otter_pda(&client, &auth, &program).await {
                            return Ok((p, auth.to_string()));
                        }
                    }
                }
                for s in SIGNER_KEYS.iter() {
                    if let Ok(p) = get_otter_pda(&client, s, &program).await {
                        return Ok((p, s.to_string()));
                    }
                }
                Err(ApiError::NotFound("no Otter-Verify PDA found".into()))
            }
        })
        .await
}

/// Used after a build failure to distinguish "real failure" from "program
/// was closed mid-build". Conservative on errors — anything other than
/// `AccountNotFound` returns false.
pub async fn is_program_buffer_missing(program_id: &Pubkey) -> bool {
    let buffer =
        Pubkey::find_program_address(&[program_id.as_ref()], &bpf_loader_upgradeable::id()).0;
    let r = get_rpc_manager()
        .execute_with_retry(|c| async move {
            match c.get_account(&buffer).await {
                Ok(_) => Ok(false),
                Err(e) => {
                    if e.to_string().contains("AccountNotFound") {
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                }
            }
        })
        .await;
    match r {
        Ok(v) => v,
        Err(e) => {
            warn!("buffer-missing check failed: {}", e);
            false
        }
    }
}

/// Appends `/tree/<commit>` when there's a commit. The literal string
/// `"None"` is treated as no commit (it shows up in old PDA data).
pub fn build_repo_url(repo: &str, commit: Option<&str>) -> String {
    match commit {
        Some(c) if !c.is_empty() && c != "None" => {
            format!("{}/tree/{}", repo.trim_end_matches('/'), c)
        }
        _ => repo.to_string(),
    }
}

/// Pulls the value out of a `<prefix> <hash>` line in `solana-verify` stdout.
pub fn extract_hash_with_prefix(text: &str, prefix: &str) -> Option<String> {
    text.lines()
        .find(|l| l.starts_with(prefix))
        .map(|l| l.trim_start_matches(prefix.trim()).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_url_with_and_without_commit() {
        assert_eq!(
            build_repo_url("https://github.com/x/y/", Some("abc")),
            "https://github.com/x/y/tree/abc"
        );
        assert_eq!(
            build_repo_url("https://github.com/x/y/", None),
            "https://github.com/x/y/"
        );
        assert_eq!(
            build_repo_url("https://github.com/x/y/", Some("")),
            "https://github.com/x/y/"
        );
    }

    #[test]
    fn extract_hash_prefix() {
        let s = "Program Hash: abc\nother";
        assert_eq!(
            extract_hash_with_prefix(s, "Program Hash:"),
            Some("abc".into())
        );
        assert_eq!(extract_hash_with_prefix(s, "Missing:"), None);
    }

    #[test]
    fn hash_strips_trailing_zeros() {
        // Empty payload (all zeros) hashes the empty string.
        let h = compute_program_hash(&[0u8; 16]);
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hash_known_bytes() {
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let mut data = b"hello".to_vec();
        data.extend_from_slice(&[0u8; 8]);
        assert_eq!(
            compute_program_hash(&data),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
