-- Actionable events and conditions derived from decisions, plans, check runs and trust
-- snapshots. One row per dedupe key: repeats raise count and last_seen instead of adding rows.
-- An empty project means the global configuration. resolved_at is set when the underlying
-- condition no longer holds; acknowledgement is the separate human-attention state.
CREATE TABLE notifications (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK(kind IN ('hook_error','observe_would_deny','plan_waiting','plan_stalled','check_failed','config_stale','config_untrusted')),
    project TEXT NOT NULL,
    subject TEXT NOT NULL,
    plan_id INTEGER,
    title TEXT NOT NULL,
    detail TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 1 CHECK(count >= 1),
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'unread' CHECK(state IN ('unread','acknowledged')),
    acknowledged_at INTEGER,
    actor TEXT CHECK(actor IN ('human','yoke')),
    resolved_at INTEGER,
    CHECK((state = 'unread') = (acknowledged_at IS NULL AND actor IS NULL))
);
CREATE INDEX notifications_project ON notifications(project,state,last_seen DESC);
