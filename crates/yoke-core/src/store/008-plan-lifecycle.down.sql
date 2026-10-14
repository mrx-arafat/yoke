-- Fails if any plan is cancelled. Run with foreign keys disabled to keep dependent rows.
DROP TABLE evidence;
CREATE TABLE plans_old (
    id INTEGER PRIMARY KEY,
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    file TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('draft','approved','building','checking','done','waiting','stalled')),
    approved_header TEXT,
    revision INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    base_ref TEXT,
    UNIQUE(project,slug)
);
INSERT INTO plans_old(id,project,slug,file,state,approved_header,revision,created_at,updated_at,base_ref)
    SELECT id,project,slug,file,state,approved_header,revision,created_at,updated_at,base_ref FROM plans;
DROP TABLE plans;
ALTER TABLE plans_old RENAME TO plans;
PRAGMA user_version=7;
