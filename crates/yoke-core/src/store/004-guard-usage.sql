CREATE TABLE stop_guard (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    blocks INTEGER NOT NULL DEFAULT 0 CHECK(blocks >= 0),
    last_fingerprint TEXT NOT NULL,
    last_block_at INTEGER NOT NULL,
    last_transition_id INTEGER NOT NULL DEFAULT 0,
    last_check_run_id INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(session_id,plan_id)
);
CREATE INDEX stop_guard_plan ON stop_guard(plan_id);
CREATE TABLE usage_samples (
    agent TEXT NOT NULL,
    session_id TEXT NOT NULL,
    project TEXT NOT NULL,
    record_key TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL CHECK(input_tokens >= 0),
    output_tokens INTEGER NOT NULL CHECK(output_tokens >= 0),
    cache_read_tokens INTEGER NOT NULL CHECK(cache_read_tokens >= 0),
    cache_write_tokens INTEGER NOT NULL CHECK(cache_write_tokens >= 0),
    PRIMARY KEY(agent,session_id,record_key)
);
CREATE INDEX usage_project ON usage_samples(project);
CREATE INDEX usage_session ON usage_samples(session_id);
