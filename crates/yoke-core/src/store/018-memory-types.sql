-- What a memory records: a plain note, or a decision, discovery, bugfix, feature, refactor or
-- change. Existing rows keep the neutral note type.
ALTER TABLE memories ADD COLUMN memory_type TEXT NOT NULL DEFAULT 'note'
    CHECK(memory_type IN ('note','decision','discovery','bugfix','feature','refactor','change'));
CREATE INDEX memories_type ON memories(project,memory_type,ts DESC);
