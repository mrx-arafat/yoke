DROP TABLE jobs;
DROP TABLE memories_fts;
CREATE VIRTUAL TABLE memories_fts USING fts5(text,content='memories',content_rowid='id');
INSERT INTO memories_fts(memories_fts) VALUES('rebuild');
PRAGMA user_version=9;
