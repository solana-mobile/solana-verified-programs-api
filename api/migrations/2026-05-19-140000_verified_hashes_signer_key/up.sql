-- Key the content-addressed directory by `(executable_hash, signer)` instead
-- of just `executable_hash`. The signer is the on-chain pubkey whose PDA
-- supplied the build params. `/status` filters claims by trust set
-- (program upgrade authority + whitelisted Otter signers), so signer
-- attribution at the directory row is what makes that filter possible.

ALTER TABLE verified_hashes ADD COLUMN signer VARCHAR;

-- Backfill: pull the signer from the original solana_program_builds row
-- by matching on (repository, commit_hash, lib_name, build_args). For rows
-- that can't be matched, fall back to the system program sentinel so the
-- migration succeeds; they won't satisfy any real trust check.
UPDATE verified_hashes vh
SET signer = COALESCE(
    (
        SELECT sp.signer
        FROM solana_program_builds sp
        WHERE sp.repository                  = vh.repository
          AND sp.commit_hash       IS NOT DISTINCT FROM vh.commit_hash
          AND sp.lib_name          IS NOT DISTINCT FROM vh.lib_name
          AND sp.base_docker_image IS NOT DISTINCT FROM vh.base_docker_image
          AND sp.mount_path        IS NOT DISTINCT FROM vh.mount_path
          AND sp.cargo_args        IS NOT DISTINCT FROM vh.cargo_args
          AND sp.bpf_flag                       = vh.bpf_flag
          AND sp.arch              IS NOT DISTINCT FROM vh.arch
          AND sp.signer IS NOT NULL
        ORDER BY sp.created_at DESC
        LIMIT 1
    ),
    '11111111111111111111111111111111'
);

ALTER TABLE verified_hashes ALTER COLUMN signer SET NOT NULL;
ALTER TABLE verified_hashes DROP CONSTRAINT verified_hashes_pkey;
ALTER TABLE verified_hashes ADD PRIMARY KEY (executable_hash, signer);

CREATE INDEX IF NOT EXISTS verified_hashes_signer_idx ON verified_hashes (signer);
