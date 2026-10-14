-- Unix time (seconds, UTC) of each usage sample, taken from its transcript line.
-- Rows imported earlier stay NULL ("time unknown") until their session is imported again.
ALTER TABLE usage_samples ADD COLUMN ts INTEGER;
CREATE INDEX usage_project_ts ON usage_samples(project,ts);
