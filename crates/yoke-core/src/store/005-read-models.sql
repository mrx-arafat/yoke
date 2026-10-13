ALTER TABLE usage_samples ADD COLUMN cost_nano_usd INTEGER CHECK(cost_nano_usd >= 0);
CREATE INDEX sessions_project_started ON sessions(project,started_at DESC);
