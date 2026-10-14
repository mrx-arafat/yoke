-- Commit the plan started from, so review still sees work committed during the plan.
ALTER TABLE plans ADD COLUMN base_ref TEXT;
