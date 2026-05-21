-- Content-addressed directory of verified builds. A row asserts that signer
-- `signer` claims `(repository, commit_hash, build_args)` deterministically
-- produces those bytes. Multiple signers may claim the same hash; each is
-- its own row.

CREATE TABLE IF NOT EXISTS verified_hashes (
    executable_hash    VARCHAR     NOT NULL,
    signer             VARCHAR(44) NOT NULL,
    repository         VARCHAR     NOT NULL,
    commit_hash        VARCHAR,
    lib_name           VARCHAR,
    base_docker_image  VARCHAR,
    mount_path         VARCHAR,
    cargo_args         TEXT[],
    bpf_flag           BOOLEAN     NOT NULL DEFAULT FALSE,
    arch               VARCHAR,
    verified_at        TIMESTAMP   NOT NULL DEFAULT NOW(),
    PRIMARY KEY (executable_hash, signer)
);

CREATE INDEX IF NOT EXISTS verified_hashes_repository_idx  ON verified_hashes (repository);
CREATE INDEX IF NOT EXISTS verified_hashes_verified_at_idx ON verified_hashes (verified_at DESC);
CREATE INDEX IF NOT EXISTS verified_hashes_signer_idx      ON verified_hashes (signer);

-- Backfill from existing verified_programs joined with the build that
-- produced them. Rows whose build is missing a signer fall back to the
-- system-program sentinel so the backfill remains key-unique.
INSERT INTO verified_hashes (
    executable_hash,
    signer,
    repository,
    commit_hash,
    lib_name,
    base_docker_image,
    mount_path,
    cargo_args,
    bpf_flag,
    arch,
    verified_at
)
SELECT DISTINCT ON (vp.executable_hash, COALESCE(sp.signer, '11111111111111111111111111111111'))
    vp.executable_hash,
    COALESCE(sp.signer, '11111111111111111111111111111111'),
    sp.repository,
    sp.commit_hash,
    sp.lib_name,
    sp.base_docker_image,
    sp.mount_path,
    sp.cargo_args,
    sp.bpf_flag,
    sp.arch,
    vp.verified_at
FROM verified_programs vp
JOIN solana_program_builds sp ON sp.id = vp.solana_build_id
WHERE vp.is_verified = true
  AND vp.executable_hash IS NOT NULL
  AND vp.executable_hash <> ''
ORDER BY vp.executable_hash,
         COALESCE(sp.signer, '11111111111111111111111111111111'),
         vp.verified_at DESC
ON CONFLICT (executable_hash, signer) DO NOTHING;

-- The program-bound verified_programs table is dead weight in the new model.
DROP TABLE IF EXISTS verified_programs;

-- The program_authority cache table was a fast-lookup for is_frozen / is_closed
-- consulted by the program-bound check_is_verified chain. In the new model
-- every read RPCs the authority live; nothing reads this table.
DROP TABLE IF EXISTS program_authority;
