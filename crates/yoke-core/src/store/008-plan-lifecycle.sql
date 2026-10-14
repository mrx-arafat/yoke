-- SQLite cannot widen a CHECK in place, so rebuild plans. The migration runner disables
-- foreign key enforcement for this step; otherwise DROP TABLE would cascade into
-- transitions, check_runs and stop_guard and clear sessions.active_plan.
-- done_fingerprint is the tree a plan was completed on; Stop compares it with the
-- current tree. Existing completed plans take their newest passing check run.
CREATE TABLE plans_new (
    id INTEGER PRIMARY KEY,
    project TEXT NOT NULL,
    slug TEXT NOT NULL,
    file TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('draft','approved','building','checking','done','waiting','stalled','cancelled')),
    approved_header TEXT,
    revision INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    base_ref TEXT,
    done_fingerprint TEXT,
    UNIQUE(project,slug)
);
INSERT INTO plans_new(id,project,slug,file,state,approved_header,revision,created_at,updated_at,base_ref,done_fingerprint)
    SELECT id,project,slug,file,state,approved_header,revision,created_at,updated_at,base_ref,
        CASE WHEN state='done' THEN (SELECT fingerprint FROM check_runs WHERE plan_id=plans.id AND result='pass' ORDER BY id DESC LIMIT 1) END
    FROM plans;
DROP TABLE plans;
ALTER TABLE plans_new RENAME TO plans;

-- SHA-256 of each declared artifact at capture time. Runs recorded before this
-- migration have no rows here and verify as unrecorded.
CREATE TABLE evidence (
    check_run_id INTEGER NOT NULL REFERENCES check_runs(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    PRIMARY KEY(check_run_id,path)
);
