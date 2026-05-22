-- Drop the v2 tables. We do not restore the legacy three-table layout — if a
-- rollback is needed, re-run the prior diesel migrations against a fresh DB.
DROP TABLE IF EXISTS builds CASCADE;
DROP TABLE IF EXISTS program_state CASCADE;
DROP TABLE IF EXISTS build_logs CASCADE;
