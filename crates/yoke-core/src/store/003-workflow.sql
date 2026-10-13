CREATE TABLE plans (
    id INTEGER PRIMARY KEY,
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    file TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('draft','approved','building','checking','done','waiting','stalled')),
    approved_header TEXT,
    revision INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(project,slug)
);
CREATE TABLE transitions (
    id INTEGER PRIMARY KEY,
    plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    ts INTEGER NOT NULL,
    from_state TEXT NOT NULL,
    to_state TEXT NOT NULL,
    actor TEXT NOT NULL CHECK(actor IN ('human','agent','yoke')),
    note TEXT
);
CREATE INDEX transitions_plan ON transitions(plan_id,id);
CREATE TABLE check_runs (
    id INTEGER PRIMARY KEY,
    plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    ts INTEGER NOT NULL,
    name TEXT NOT NULL,
    command TEXT NOT NULL,
    exit_code INTEGER,
    output_tail TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    blocking INTEGER NOT NULL CHECK(blocking IN (0,1)),
    ms INTEGER NOT NULL CHECK(ms >= 0),
    result TEXT NOT NULL,
    findings TEXT NOT NULL DEFAULT '[]',
    artifacts TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX check_runs_plan ON check_runs(plan_id,id);
-- Core never populated its TEXT placeholder; retain any unexpected legacy value.
ALTER TABLE sessions RENAME COLUMN active_plan TO core_active_plan_legacy;
ALTER TABLE sessions ADD COLUMN active_plan INTEGER REFERENCES plans(id) ON DELETE SET NULL;
CREATE INDEX sessions_active_plan ON sessions(active_plan);
