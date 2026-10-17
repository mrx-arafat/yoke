-- Indexes for a state database that holds months of history. Measured on a copy with 500k
-- decisions and 200k facts, each one removes a scan of the whole table.
-- The newest decision of one agent (yoke health): without it SQLite scanned and sorted every decision.
CREATE INDEX decisions_agent ON decisions(agent,ts DESC,id DESC);
-- yoke trial report: both aggregates read this index alone instead of every decision row,
-- payload included. The order is the GROUP BY order of the report.
CREATE INDEX decisions_trial ON decisions(agent,mode,event,outcome,ts,session_id);
-- Retention: find facts older than the boundary without reading every fact.
CREATE INDEX facts_ts ON facts(ts);
