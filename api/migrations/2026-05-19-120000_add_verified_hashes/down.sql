-- Restore program_authority (cache table; no data backfill).
CREATE TABLE IF NOT EXISTS program_authority (
    program_id   VARCHAR(44) NOT NULL,
    authority_id VARCHAR(44),
    last_updated TIMESTAMP NOT NULL DEFAULT NOW(),
    is_frozen    BOOLEAN NOT NULL DEFAULT FALSE,
    is_closed    BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (program_id)
);

-- Restore verified_programs (without data; the forward backfill is not
-- generally reversible — directory rows don't map cleanly back to the
-- program-bound shape).
CREATE TABLE IF NOT EXISTS verified_programs (
    id              VARCHAR(36) PRIMARY KEY,
    program_id      VARCHAR(44) NOT NULL,
    is_verified     BOOLEAN     NOT NULL,
    on_chain_hash   VARCHAR     NOT NULL,
    executable_hash VARCHAR     NOT NULL,
    verified_at     TIMESTAMP   NOT NULL DEFAULT NOW(),
    solana_build_id VARCHAR(36) NOT NULL,
    FOREIGN KEY (solana_build_id) REFERENCES solana_program_builds (id)
);

CREATE INDEX IF NOT EXISTS verified_programs_program_id_idx ON verified_programs (program_id);
CREATE INDEX IF NOT EXISTS verified_programs_solana_build_id_idx ON verified_programs (solana_build_id);
CREATE INDEX IF NOT EXISTS idx_verified_programs_program_id_is_verified ON verified_programs(program_id, is_verified);

DROP INDEX IF EXISTS verified_hashes_signer_idx;
DROP INDEX IF EXISTS verified_hashes_verified_at_idx;
DROP INDEX IF EXISTS verified_hashes_repository_idx;
DROP TABLE IF EXISTS verified_hashes;
