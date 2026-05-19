-- The program-bound `verified_programs` table is dead weight in the
-- content-addressed model. `/status/:address` answers freshness by looking
-- the current on-chain hash up in `verified_hashes`, so there is nothing
-- left to store here.

DROP TABLE IF EXISTS verified_programs;
