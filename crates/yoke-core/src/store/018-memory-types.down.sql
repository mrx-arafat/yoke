-- Test-only reversal in disposable state. The type of every memory is lost.
DROP INDEX memories_type;
ALTER TABLE memories DROP COLUMN memory_type;
PRAGMA user_version=17;
