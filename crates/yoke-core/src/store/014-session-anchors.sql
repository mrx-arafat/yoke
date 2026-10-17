-- Where each agent session started, and the project root it was given there. A session keeps that
-- one project whichever directory its later tool calls report, so a policy, a plan and the
-- memory of a session cannot be moved by changing directory.
CREATE TABLE session_anchors (
    agent TEXT NOT NULL,
    session_id TEXT NOT NULL,
    anchor TEXT NOT NULL,
    root TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(agent,session_id)
) WITHOUT ROWID;
CREATE INDEX session_anchors_created ON session_anchors(created_at);
