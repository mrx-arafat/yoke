CREATE TABLE facts (
    id INTEGER PRIMARY KEY,
    identity TEXT NOT NULL UNIQUE DEFAULT (lower(hex(randomblob(16)))),
    ts INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    project TEXT NOT NULL,
    kind TEXT NOT NULL,
    detail TEXT NOT NULL,
    digested INTEGER NOT NULL DEFAULT 0 CHECK(digested IN (0,1))
);
CREATE INDEX facts_session ON facts(session_id,project,digested,id);
CREATE INDEX facts_project ON facts(project,ts DESC);
CREATE TABLE memories (
    id INTEGER PRIMARY KEY,
    identity TEXT NOT NULL UNIQUE DEFAULT (lower(hex(randomblob(16)))),
    ts INTEGER NOT NULL,
    project TEXT NOT NULL,
    source TEXT NOT NULL CHECK(source IN ('fact','remember','digest')),
    text TEXT NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN (0,1)),
    shared INTEGER NOT NULL DEFAULT 0 CHECK(shared IN (0,1))
);
CREATE INDEX memories_project ON memories(project,pinned DESC,ts DESC);
CREATE TABLE memory_facts (
    memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    PRIMARY KEY(memory_id,fact_id)
);
CREATE INDEX memory_facts_fact ON memory_facts(fact_id);
CREATE TABLE imported_identities (
    identity TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('memory','fact')),
    local_id INTEGER NOT NULL,
    PRIMARY KEY(identity,kind)
);
CREATE VIRTUAL TABLE memories_fts USING fts5(text,content='memories',content_rowid='id');
CREATE TRIGGER memories_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid,text) VALUES(new.id,new.text);
END;
CREATE TRIGGER memories_delete AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts,rowid,text) VALUES('delete',old.id,old.text);
    DELETE FROM imported_identities WHERE identity=old.identity AND kind='memory';
END;
CREATE TRIGGER memories_update AFTER UPDATE OF text ON memories BEGIN
    INSERT INTO memories_fts(memories_fts,rowid,text) VALUES('delete',old.id,old.text);
    INSERT INTO memories_fts(rowid,text) VALUES(new.id,new.text);
END;
