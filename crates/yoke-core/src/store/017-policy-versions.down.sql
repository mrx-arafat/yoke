-- Test-only reversal in disposable state. Historical configuration archives are lost.
ALTER TABLE decisions DROP COLUMN policy_hash;
DROP TABLE policy_versions;
PRAGMA user_version=16;
