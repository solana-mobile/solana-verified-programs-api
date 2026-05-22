CREATE INDEX IF NOT EXISTS idx_verified_programs_executable_hash
    ON verified_programs(executable_hash)
    WHERE is_verified = true;
