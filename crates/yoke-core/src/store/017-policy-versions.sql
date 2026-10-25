-- Historical policy is captured by hooks from the bytes they loaded, never inferred from
-- today's trust table. Existing decisions deliberately retain a NULL policy_hash.
CREATE TABLE policy_versions (
    sha256 TEXT PRIMARY KEY NOT NULL,
    content TEXT NOT NULL
);
ALTER TABLE decisions ADD COLUMN policy_hash TEXT REFERENCES policy_versions(sha256);
