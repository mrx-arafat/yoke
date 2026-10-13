DROP INDEX sessions_project_started;
ALTER TABLE usage_samples DROP COLUMN cost_nano_usd;
PRAGMA user_version=4;
