DROP TRIGGER decisions_count_move;
DROP TRIGGER decisions_count_delete;
DROP TRIGGER decisions_count_insert;
ALTER TABLE sessions DROP COLUMN decision_count;
PRAGMA user_version=5;
