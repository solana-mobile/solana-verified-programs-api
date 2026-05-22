//! Everything that touches the Solana chain.

use crate::{config::CONFIG, error::ApiError, error::Result, rpc::rpc};
use borsh::{BorshDeserialize, BorshSerialize};
use solana_account_decoder::parse_bpf_loader::{
    parse_bpf_upgradeable_loader, BpfUpgradeableLoaderAccountType, UiProgram, UiProgramData,
};
use solana_client::{
    nonblocking::rpc_client::RpcClient, rpc_client::GetConfirmedSignaturesForAddress2Config,
    rpc_config::RpcTransactionConfig,
};
use solana_sdk::{pubkey::Pubkey, signature::Signature};
use solana_sdk_ids::{bpf_loader_upgradeable, system_program};
use solana_transaction_status::{EncodedTransaction, UiMessage, UiTransactionEncoding};
use std::{str::FromStr, sync::Arc};
use tokio::process::Command;
use tracing::{error, warn};

pub const OTTER_VERIFY_PROGRAM_ID: Pubkey =
    solana_sdk::pubkey!("verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC");

/// Whitelisted Otter Verify signers, tried in order when no explicit signer
/// or program-authority claim exists.
pub const SIGNER_KEYS: [Pubkey; 3] = [
    solana_sdk::pubkey!("9VWiUUhgNoRwTH5NVehYJEDwcotwYX3VgW4MChiHPAqU"),
    solana_sdk::pubkey!("CyJj5ejJAUveDXnLduJbkvwjxcmWJNqCuB9DR7AExrHn"),
    solana_sdk::pubkey!("5vJwnLeyjV8uNJSp1zn7VLW8GwiQbcsQbGaVSwRmkE4r"),
];

// Squads-frozen programs route the final upgrade through the Squads multisig;
// the burned authority is the 5th account on this specific instruction.
const SQUADS_PROGRAM_ID: &str = "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf";
const SQUADS_AUTHORITY_IX_DATA: &str = "ZTNTtVtnvbC";
const SQUADS_AUTHORITY_ACCOUNT_INDEX: usize = 4;

/// Authority + flags fetched from chain (the non-hash half of `program_state`).
#[derive(Debug, Clone)]
pub struct ProgramOnchainState {
    pub authority: Option<String>,
    pub is_frozen: bool,
    pub is_closed: bool,
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

/// Looks up `(authority, is_frozen, is_closed)` for a program. For frozen
/// programs the authority is recovered from the last transaction on the
/// program-data PDA (Squads-shaped freezes are decoded specially).
pub async fn get_program_state(program_id: &Pubkey) -> Result<ProgramOnchainState> {
    rpc()
        .run(|c| {
            let pid = *program_id;
            async move { fetch_program_state(c, &pid).await }
        })
        .await
}

async fn fetch_program_state(
    client: Arc<RpcClient>,
    program_id: &Pubkey,
) -> Result<ProgramOnchainState> {
    let program_bytes = client
        .get_account_data(program_id)
        .await
        .map_err(|e| ApiError::Rpc(format!("fetch program account: {e}")))?;

    let program_data_pda = match parse_bpf_upgradeable_loader(&program_bytes)? {
        BpfUpgradeableLoaderAccountType::Program(UiProgram { program_data }) => {
            Pubkey::from_str(&program_data)?
        }
        other => {
            return Err(ApiError::Rpc(format!(
                "expected Program account, got: {other:?}"
            )))
        }
    };

    match client.get_account_data(&program_data_pda).await {
        Ok(bytes) => {
            if let BpfUpgradeableLoaderAccountType::ProgramData(UiProgramData {
                authority, ..
            }) = parse_bpf_upgradeable_loader(&bytes)?
            {
                if authority.is_some() {
                    return Ok(ProgramOnchainState {
                        authority,
                        is_frozen: false,
                        is_closed: false,
                    });
                }
            }
        }
        Err(e) => {
            if is_account_closed(&client, &program_data_pda)
                .await
                .unwrap_or(false)
            {
                return Ok(ProgramOnchainState {
                    authority: None,
                    is_frozen: false,
                    is_closed: true,
                });
            }
            return Err(ApiError::Rpc(format!("fetch program data: {e}")));
        }
    }

    let cfg = GetConfirmedSignaturesForAddress2Config {
        limit: Some(1),
        before: None,
        until: None,
        commitment: None,
    };
    let sigs = match client
        .get_signatures_for_address_with_config(&program_data_pda, cfg)
        .await
    {
        Ok(s) => s,
        Err(_) => {
            return Ok(ProgramOnchainState {
                authority: None,
                is_frozen: false,
                is_closed: false,
            })
        }
    };
    let Some(latest) = sigs.first() else {
        return Ok(ProgramOnchainState {
            authority: None,
            is_frozen: false,
            is_closed: false,
        });
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
            if raw.account_keys.iter().any(|k| k == SQUADS_PROGRAM_ID) {
                let squads_idx = raw
                    .account_keys
                    .iter()
                    .position(|k| k == SQUADS_PROGRAM_ID)
                    .unwrap() as u8;
                for ix in &raw.instructions {
                    if ix.program_id_index == squads_idx && ix.data == SQUADS_AUTHORITY_IX_DATA {
                        let aidx = ix.accounts[SQUADS_AUTHORITY_ACCOUNT_INDEX] as usize;
                        return Ok(ProgramOnchainState {
                            authority: Some(raw.account_keys[aidx].clone()),
                            is_frozen: true,
                            is_closed: false,
                        });
                    }
                }
            }
            return Ok(ProgramOnchainState {
                authority: Some(raw.account_keys[0].clone()),
                is_frozen: true,
                is_closed: false,
            });
        }
    }
    Ok(ProgramOnchainState {
        authority: None,
        is_frozen: false,
        is_closed: false,
    })
}

/// "Closed" covers both shapes the BPF loader leaves behind: account gone
/// (`AccountNotFound`) and zero-lamport account owned by the system program.
async fn is_account_closed(client: &RpcClient, pubkey: &Pubkey) -> Result<bool> {
    match client.get_account(pubkey).await {
        Ok(a) => Ok(a.lamports == 0 && a.owner == system_program::ID),
        Err(e) => {
            if e.to_string().contains("AccountNotFound") {
                Ok(true)
            } else {
                Err(e.into())
            }
        }
    }
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

    rpc()
        .run(move |client| {
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

/// Shells out to `solana-verify get-program-hash`. Retries transient
/// failures 3x with exponential backoff; "program appears to be closed"
/// returns immediately since the caller wants to act on it.
pub async fn get_on_chain_hash(program_id: &str) -> Result<String> {
    let mut cmd = Command::new("solana-verify");
    cmd.arg("get-program-hash")
        .arg(program_id)
        .arg("--url")
        .arg(&CONFIG.rpc_url);

    for attempt in 1..=3 {
        match exec_hash_cmd(&mut cmd).await {
            Ok(h) => return Ok(h),
            Err(e) => {
                if e.to_string().contains("Program appears to be closed") {
                    return Err(e);
                }
                error!("attempt {}/3 get-program-hash failed: {}", attempt, e);
                if attempt < 3 {
                    tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
                }
            }
        }
    }
    Err(ApiError::Rpc(
        "get-program-hash failed after 3 attempts".into(),
    ))
}

async fn exec_hash_cmd(cmd: &mut Command) -> Result<String> {
    let out = cmd
        .output()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("Could not find program data") {
            return Err(ApiError::NotFound(
                "Program appears to be closed - program data account not found".into(),
            ));
        }
        return Err(ApiError::Rpc(format!("command failed: {stderr}")));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .last()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::Internal("no hash in command output".into()))
}

/// Used after a build failure to distinguish "real failure" from "program
/// was closed mid-build". Conservative on errors — anything other than
/// `AccountNotFound` returns false.
pub async fn is_program_buffer_missing(program_id: &Pubkey) -> bool {
    let buffer =
        Pubkey::find_program_address(&[program_id.as_ref()], &bpf_loader_upgradeable::id()).0;
    let r = rpc()
        .run(|c| async move {
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
}
