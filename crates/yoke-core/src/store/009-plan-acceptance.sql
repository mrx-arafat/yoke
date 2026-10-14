-- Completion of tasks and human acceptance of requirements declared in an approved plan
-- header. `definition` is the SHA-256 of the approved item, so evidence recorded for one
-- definition never counts for an edited one.
CREATE TABLE plan_completions (
    id INTEGER PRIMARY KEY,
    plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('task','requirement')),
    item TEXT NOT NULL,
    definition TEXT NOT NULL,
    ts INTEGER NOT NULL,
    actor TEXT NOT NULL CHECK(actor IN ('human','agent')),
    evidence TEXT,
    UNIQUE(plan_id,kind,item,definition)
);
-- Pre-approval reviewer findings, anchored comments and their replies. Only top-level
-- items carry a state, and only a human closes them.
CREATE TABLE plan_feedback (
    id INTEGER PRIMARY KEY,
    plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    parent_id INTEGER REFERENCES plan_feedback(id) ON DELETE CASCADE,
    ts INTEGER NOT NULL,
    author TEXT NOT NULL CHECK(author IN ('human','agent','reviewer')),
    severity TEXT CHECK(severity IN ('blocking','advice')),
    anchor TEXT NOT NULL,
    digest TEXT NOT NULL,
    text TEXT NOT NULL,
    state TEXT CHECK(state IN ('open','resolved','acknowledged')),
    resolved_by TEXT CHECK(resolved_by IN ('human')),
    resolved_at INTEGER,
    note TEXT,
    CHECK((parent_id IS NULL) = (state IS NOT NULL))
);
CREATE INDEX plan_feedback_plan ON plan_feedback(plan_id,id);
