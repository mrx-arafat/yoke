DROP INDEX notifications_project;
ALTER TABLE notifications RENAME TO notifications_previous;
CREATE TABLE notifications (
 id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE,
 kind TEXT NOT NULL CHECK(kind IN ('hook_error','observe_would_deny','plan_waiting','plan_stalled','check_failed','config_stale','config_untrusted','usage_partial')),
 project TEXT NOT NULL, subject TEXT NOT NULL, plan_id INTEGER, title TEXT NOT NULL, detail TEXT NOT NULL,
 count INTEGER NOT NULL DEFAULT 1 CHECK(count>=1),first_seen INTEGER NOT NULL,last_seen INTEGER NOT NULL,
 state TEXT NOT NULL DEFAULT 'unread' CHECK(state IN ('unread','acknowledged')),acknowledged_at INTEGER,
 actor TEXT CHECK(actor IN ('human','yoke')),resolved_at INTEGER,
 CHECK((state='unread')=(acknowledged_at IS NULL AND actor IS NULL))
);
INSERT INTO notifications SELECT * FROM notifications_previous WHERE kind<>'usage_limit';
DROP TABLE notifications_previous;
CREATE INDEX notifications_project ON notifications(project,state,last_seen DESC);
PRAGMA user_version=15;
