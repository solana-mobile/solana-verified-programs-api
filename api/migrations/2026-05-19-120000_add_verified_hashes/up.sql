-- Content-addressed directory of verified builds.
-- Primary key is the executable_hash, so the row is a claim that a given
-- (repository, commit_hash, build_args) deterministically produces those bytes.
-- No program_id, no is_verified flag, no staleness: a row exists iff some
-- build config reproduces the hash.

CREATE TABLE IF NOT EXISTS verified_hashes (
    executable_hash VARCHAR PRIMARY KEY,
    repository VARCHAR NOT NULL,
    commit_hash VARCHAR,
    lib_name VARCHAR,
    base_docker_image VARCHAR,
    mount_path VARCHAR,
    cargo_args TEXT[],
    bpf_flag BOOLEAN NOT NULL DEFAULT FALSE,
    arch VARCHAR,
    verified_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS verified_hashes_repository_idx ON verified_hashes (repository);
CREATE INDEX IF NOT EXISTS verified_hashes_verified_at_idx ON verified_hashes (verified_at DESC);

-- Backfill from existing verified_programs joined with solana_program_builds.
-- For each distinct executable_hash, take the most recent verified build.
INSERT INTO verified_hashes (
    executable_hash,
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
SELECT DISTINCT ON (vp.executable_hash)
    vp.executable_hash,
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
ORDER BY vp.executable_hash, vp.verified_at DESC
ON CONFLICT (executable_hash) DO NOTHING;
