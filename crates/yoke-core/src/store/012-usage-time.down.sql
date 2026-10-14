DROP INDEX usage_project_ts;
ALTER TABLE usage_samples DROP COLUMN ts;
PRAGMA user_version=11;
