CREATE TABLE builds (
    id                UUID PRIMARY KEY,
    repository        TEXT NOT NULL,
    commit_hash       TEXT,
    program_id        TEXT NOT NULL,
    lib_name          TEXT,
    base_docker_image TEXT,
    mount_path        TEXT,
    cargo_args        TEXT[],
    bpf_flag          BOOLEAN NOT NULL DEFAULT FALSE,
    arch              TEXT,
    signer            TEXT,
    status            TEXT NOT NULL CHECK (status IN ('in_progress', 'completed', 'failed')),
    executable_hash   TEXT,
    error_message     TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at      TIMESTAMPTZ
);

CREATE INDEX builds_executable_hash_idx ON builds (executable_hash) WHERE status = 'completed';
CREATE INDEX builds_program_id_created_idx ON builds (program_id, created_at DESC);
CREATE INDEX builds_program_completed_idx ON builds (program_id, completed_at DESC) WHERE status = 'completed';

CREATE TABLE program_state (
    program_id    TEXT PRIMARY KEY,
    on_chain_hash TEXT,
    authority     TEXT,
    is_frozen     BOOLEAN NOT NULL DEFAULT FALSE,
    is_closed     BOOLEAN NOT NULL DEFAULT FALSE,
    last_checked  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX program_state_last_checked_idx ON program_state (last_checked ASC);

CREATE TABLE build_logs (
    id           UUID PRIMARY KEY,
    program_id   TEXT NOT NULL,
    file_name    TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX build_logs_program_idx ON build_logs (program_id, created_at DESC);
