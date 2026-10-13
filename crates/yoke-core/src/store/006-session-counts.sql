ALTER TABLE sessions ADD COLUMN decision_count INTEGER NOT NULL DEFAULT 0 CHECK(decision_count >= 0);
UPDATE sessions SET decision_count = (SELECT count(*) FROM decisions WHERE decisions.session_id = sessions.id);

CREATE TRIGGER decisions_count_insert AFTER INSERT ON decisions BEGIN
    UPDATE sessions SET decision_count = decision_count + 1 WHERE id = new.session_id;
END;
CREATE TRIGGER decisions_count_delete AFTER DELETE ON decisions BEGIN
    UPDATE sessions SET decision_count = decision_count - 1 WHERE id = old.session_id;
END;
CREATE TRIGGER decisions_count_move AFTER UPDATE OF session_id ON decisions
WHEN old.session_id != new.session_id BEGIN
    UPDATE sessions SET decision_count = decision_count - 1 WHERE id = old.session_id;
    UPDATE sessions SET decision_count = decision_count + 1 WHERE id = new.session_id;
END;
