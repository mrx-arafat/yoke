DROP INDEX sessions_active_plan;
ALTER TABLE sessions DROP COLUMN active_plan;
ALTER TABLE sessions RENAME COLUMN core_active_plan_legacy TO active_plan;
DROP TABLE check_runs;
DROP TABLE transitions;
DROP TABLE plans;
PRAGMA user_version=2;
