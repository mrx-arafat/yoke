-- Durable background work. At most one queued, running or retry-pending job per dedupe key.
CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK(length(kind) BETWEEN 1 AND 64),
    dedupe_key TEXT NOT NULL,
    project TEXT NOT NULL,
    payload TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','running','succeeded','failed','dead')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
    max_attempts INTEGER NOT NULL CHECK(max_attempts >= 1),
    run_after INTEGER NOT NULL,
    owner_pid INTEGER,
    lease_expires INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX jobs_active_key ON jobs(dedupe_key) WHERE state IN ('queued','running','failed');
CREATE INDEX jobs_state ON jobs(state,run_after);

-- Stem English words so plurals and verb forms match; the index is rebuilt from memories.
DROP TABLE memories_fts;
CREATE VIRTUAL TABLE memories_fts USING fts5(text,content='memories',content_rowid='id',tokenize='porter unicode61');
INSERT INTO memories_fts(memories_fts) VALUES('rebuild');
