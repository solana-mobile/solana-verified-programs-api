-- Transition the legacy three-table layout to the v2 layout, preserving data.
--   solana_program_builds + verified_programs → builds (executable_hash and
--   completed_at folded onto the build).
--   program_authority → program_state (renamed columns, plus on_chain_hash
--   populated from the latest verified_programs row per program).

-- 1. Stage the new columns on the build table before we drop verified_programs.

ALTER TABLE solana_program_builds ADD COLUMN executable_hash TEXT;
ALTER TABLE solana_program_builds ADD COLUMN error_message    TEXT;
ALTER TABLE solana_program_builds ADD COLUMN completed_at     TIMESTAMPTZ;

UPDATE solana_program_builds spb
SET executable_hash = vp.executable_hash,
    completed_at   = vp.verified_at AT TIME ZONE 'UTC'
FROM verified_programs vp
WHERE vp.solana_build_id = spb.id;

-- 2. program_authority → program_state, with on_chain_hash added and populated
--    from the most recent verified_programs row per program.

ALTER TABLE program_authority RENAME TO program_state;
ALTER TABLE program_state RENAME COLUMN authority_id TO authority;
ALTER TABLE program_state RENAME COLUMN last_updated TO last_checked;
ALTER TABLE program_state ADD COLUMN on_chain_hash TEXT;

UPDATE program_state ps
SET on_chain_hash = vp.on_chain_hash
FROM (
    SELECT DISTINCT ON (program_id) program_id, on_chain_hash
    FROM verified_programs
    ORDER BY program_id, verified_at DESC
) vp
WHERE vp.program_id = ps.program_id;

-- Programs that had a verified build but no program_authority row need a
-- program_state row so /status can resolve their on-chain hash.
INSERT INTO program_state (program_id, on_chain_hash, is_frozen, is_closed, last_checked)
SELECT DISTINCT ON (vp.program_id) vp.program_id, vp.on_chain_hash, FALSE, FALSE, NOW()
FROM verified_programs vp
WHERE NOT EXISTS (SELECT 1 FROM program_state ps WHERE ps.program_id = vp.program_id)
ORDER BY vp.program_id, vp.verified_at DESC;

-- 3. verified_programs is now redundant. Dropping it also releases the FK
--    that would block the build id → UUID conversion below.

DROP TABLE verified_programs;

-- 4. Rename solana_program_builds → builds and normalise legacy statuses.

ALTER TABLE solana_program_builds RENAME TO builds;
UPDATE builds SET status = 'failed' WHERE status = 'un-used';

-- 5. Widen column types: VARCHAR(N)/VARCHAR → TEXT, TIMESTAMP → TIMESTAMPTZ
--    (assuming UTC for the in-place values), text id → UUID.

ALTER TABLE builds ALTER COLUMN id                TYPE UUID USING id::uuid;
ALTER TABLE builds ALTER COLUMN repository        TYPE TEXT;
ALTER TABLE builds ALTER COLUMN commit_hash       TYPE TEXT;
ALTER TABLE builds ALTER COLUMN program_id        TYPE TEXT;
ALTER TABLE builds ALTER COLUMN lib_name          TYPE TEXT;
ALTER TABLE builds ALTER COLUMN base_docker_image TYPE TEXT;
ALTER TABLE builds ALTER COLUMN mount_path        TYPE TEXT;
ALTER TABLE builds ALTER COLUMN status            TYPE TEXT;
ALTER TABLE builds ALTER COLUMN signer            TYPE TEXT;
ALTER TABLE builds ALTER COLUMN arch              TYPE TEXT;
ALTER TABLE builds ALTER COLUMN created_at        TYPE TIMESTAMPTZ USING created_at AT TIME ZONE 'UTC';

ALTER TABLE program_state ALTER COLUMN program_id   TYPE TEXT;
ALTER TABLE program_state ALTER COLUMN authority    TYPE TEXT;
ALTER TABLE program_state ALTER COLUMN last_checked TYPE TIMESTAMPTZ USING last_checked AT TIME ZONE 'UTC';

ALTER TABLE build_logs RENAME COLUMN program_address TO program_id;
ALTER TABLE build_logs ALTER COLUMN id         TYPE UUID USING id::uuid;
ALTER TABLE build_logs ALTER COLUMN program_id TYPE TEXT;
ALTER TABLE build_logs ALTER COLUMN file_name  TYPE TEXT;
ALTER TABLE build_logs ALTER COLUMN created_at TYPE TIMESTAMPTZ USING created_at AT TIME ZONE 'UTC';

-- 6. Status CHECK matches the v2 enum-of-three (the 'un-used' value is gone
--    above).

ALTER TABLE builds DROP CONSTRAINT IF EXISTS solana_program_builds_status_check;
ALTER TABLE builds ADD CONSTRAINT builds_status_check
    CHECK (status IN ('in_progress', 'completed', 'failed'));

-- 7. Replace legacy indexes with v2 ones. Anything that referenced the now-
--    dropped verified_programs is already gone; the rest gets renamed.

DROP INDEX IF EXISTS solana_program_builds_program_id_idx;
DROP INDEX IF EXISTS solana_program_builds_id_idx;
DROP INDEX IF EXISTS program_authority_program_id_index;
DROP INDEX IF EXISTS idx_solana_program_builds_created_at;
DROP INDEX IF EXISTS idx_program_authority_updated_at;
DROP INDEX IF EXISTS idx_solana_program_builds_program_status;
DROP INDEX IF EXISTS idx_solana_program_builds_signer;
DROP INDEX IF EXISTS idx_build_logs_program_created;
DROP INDEX IF EXISTS idx_solana_builds_duplicate_check;

CREATE INDEX builds_executable_hash_idx     ON builds (executable_hash)              WHERE status = 'completed';
CREATE INDEX builds_program_id_created_idx  ON builds (program_id, created_at DESC);
CREATE INDEX builds_program_completed_idx   ON builds (program_id, completed_at DESC) WHERE status = 'completed';
CREATE INDEX program_state_last_checked_idx ON program_state (last_checked ASC);
CREATE INDEX build_logs_program_idx         ON build_logs (program_id, created_at DESC);
