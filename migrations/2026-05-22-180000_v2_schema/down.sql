-- Best-effort rollback. We can recover the table shape but not the per-row
-- verified_programs entries — the FK linking each verify to its build was
-- dropped on the way up, and the merged columns don't carry per-verify
-- timestamps. verified_programs is recreated empty.

DROP INDEX IF EXISTS builds_executable_hash_idx;
DROP INDEX IF EXISTS builds_program_id_created_idx;
DROP INDEX IF EXISTS builds_program_completed_idx;
DROP INDEX IF EXISTS program_state_last_checked_idx;
DROP INDEX IF EXISTS build_logs_program_idx;

ALTER TABLE builds DROP CONSTRAINT IF EXISTS builds_status_check;
ALTER TABLE builds RENAME TO solana_program_builds;

ALTER TABLE solana_program_builds ALTER COLUMN id                TYPE VARCHAR(36) USING id::text;
ALTER TABLE solana_program_builds ALTER COLUMN repository        TYPE VARCHAR;
ALTER TABLE solana_program_builds ALTER COLUMN commit_hash       TYPE VARCHAR;
ALTER TABLE solana_program_builds ALTER COLUMN program_id        TYPE VARCHAR(44);
ALTER TABLE solana_program_builds ALTER COLUMN lib_name          TYPE VARCHAR;
ALTER TABLE solana_program_builds ALTER COLUMN base_docker_image TYPE VARCHAR;
ALTER TABLE solana_program_builds ALTER COLUMN mount_path        TYPE VARCHAR;
ALTER TABLE solana_program_builds ALTER COLUMN status            TYPE VARCHAR(20);
ALTER TABLE solana_program_builds ALTER COLUMN signer            TYPE VARCHAR;
ALTER TABLE solana_program_builds ALTER COLUMN arch              TYPE VARCHAR(3);
ALTER TABLE solana_program_builds ALTER COLUMN created_at        TYPE TIMESTAMP;
ALTER TABLE solana_program_builds DROP COLUMN executable_hash;
ALTER TABLE solana_program_builds DROP COLUMN error_message;
ALTER TABLE solana_program_builds DROP COLUMN completed_at;
ALTER TABLE solana_program_builds ADD CONSTRAINT solana_program_builds_status_check
    CHECK (status IN ('in_progress', 'completed', 'failed', 'un-used'));

ALTER TABLE program_state RENAME TO program_authority;
ALTER TABLE program_authority RENAME COLUMN authority    TO authority_id;
ALTER TABLE program_authority RENAME COLUMN last_checked TO last_updated;
ALTER TABLE program_authority DROP COLUMN on_chain_hash;
ALTER TABLE program_authority ALTER COLUMN program_id   TYPE VARCHAR(44);
ALTER TABLE program_authority ALTER COLUMN authority_id TYPE VARCHAR(44);
ALTER TABLE program_authority ALTER COLUMN last_updated TYPE TIMESTAMP;

ALTER TABLE build_logs RENAME COLUMN program_id TO program_address;
ALTER TABLE build_logs ALTER COLUMN id              TYPE VARCHAR(36) USING id::text;
ALTER TABLE build_logs ALTER COLUMN program_address TYPE VARCHAR(44);
ALTER TABLE build_logs ALTER COLUMN file_name       TYPE VARCHAR;
ALTER TABLE build_logs ALTER COLUMN created_at      TYPE TIMESTAMP;

CREATE TABLE verified_programs (
    id VARCHAR(36) PRIMARY KEY,
    program_id VARCHAR(44) NOT NULL,
    is_verified BOOLEAN NOT NULL,
    on_chain_hash VARCHAR NOT NULL,
    executable_hash VARCHAR NOT NULL,
    verified_at TIMESTAMP NOT NULL DEFAULT NOW(),
    solana_build_id VARCHAR(36) NOT NULL,
    FOREIGN KEY (solana_build_id) REFERENCES solana_program_builds (id)
);
