CREATE TABLE trust (
    scope TEXT NOT NULL CHECK(scope IN ('global','project')),
    root TEXT NOT NULL,
    file TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    content TEXT NOT NULL,
    trusted_at INTEGER NOT NULL,
    PRIMARY KEY(scope,root,file)
);
CREATE TABLE sessions (
    id TEXT PRIMARY KEY NOT NULL,
    agent TEXT NOT NULL,
    project TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    active_plan TEXT
);
CREATE TABLE decisions (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE RESTRICT,
    agent TEXT NOT NULL,
    project TEXT NOT NULL,
    event TEXT NOT NULL,
    tool TEXT,
    mode TEXT NOT NULL CHECK(mode IN ('observe','enforce')),
    outcome TEXT NOT NULL CHECK(outcome IN ('allow','deny','would_deny','rewrite','error')),
    reason TEXT,
    gates TEXT NOT NULL,
    payload TEXT NOT NULL
);
CREATE INDEX decisions_ts ON decisions(ts DESC,id DESC);
CREATE INDEX decisions_project ON decisions(project,ts DESC);
CREATE INDEX decisions_session ON decisions(session_id,ts DESC);
CREATE INDEX decisions_outcome ON decisions(outcome,ts DESC);
CREATE INDEX decisions_tool ON decisions(tool,ts DESC);
CREATE TABLE kv (key TEXT PRIMARY KEY NOT NULL,value TEXT NOT NULL);
