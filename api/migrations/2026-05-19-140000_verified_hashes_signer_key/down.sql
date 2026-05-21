DROP INDEX IF EXISTS verified_hashes_signer_idx;
ALTER TABLE verified_hashes DROP CONSTRAINT verified_hashes_pkey;
ALTER TABLE verified_hashes ADD PRIMARY KEY (executable_hash);
ALTER TABLE verified_hashes DROP COLUMN signer;
