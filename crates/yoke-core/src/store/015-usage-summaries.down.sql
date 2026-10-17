DROP TRIGGER usage_totals_update;
DROP TRIGGER usage_totals_delete;
DROP TRIGGER usage_totals_insert;
DROP TABLE usage_day_totals;
DROP TABLE usage_session_models;
DROP TABLE usage_session_totals;
DROP INDEX usage_session_time;
DROP INDEX usage_time;
-- Partial-usage notifications are derived and cannot be represented by schema 14.
DROP INDEX notifications_project;
ALTER TABLE notifications RENAME TO notifications_previous;
CREATE TABLE notifications (
 id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE,
 kind TEXT NOT NULL CHECK(kind IN ('hook_error','observe_would_deny','plan_waiting','plan_stalled','check_failed','config_stale','config_untrusted')),
 project TEXT NOT NULL, subject TEXT NOT NULL, plan_id INTEGER, title TEXT NOT NULL, detail TEXT NOT NULL,
 count INTEGER NOT NULL DEFAULT 1 CHECK(count>=1),first_seen INTEGER NOT NULL,last_seen INTEGER NOT NULL,
 state TEXT NOT NULL DEFAULT 'unread' CHECK(state IN ('unread','acknowledged')),acknowledged_at INTEGER,
 actor TEXT CHECK(actor IN ('human','yoke')),resolved_at INTEGER,
 CHECK((state='unread')=(acknowledged_at IS NULL AND actor IS NULL))
);
INSERT INTO notifications(id,key,kind,project,subject,plan_id,title,detail,count,first_seen,last_seen,state,acknowledged_at,actor,resolved_at) SELECT id,key,kind,project,subject,plan_id,title,detail,count,first_seen,last_seen,state,acknowledged_at,actor,resolved_at FROM notifications_previous WHERE kind<>'usage_partial';
DROP TABLE notifications_previous;
CREATE INDEX notifications_project ON notifications(project,state,last_seen DESC);
PRAGMA user_version=14;
