-- Why a job is paused or blocked, as a machine-readable code, so the
-- scheduler knows which holds it may lift by itself (a day rolling over, a
-- connector being fixed, the app restarting) and which only the user can.
ALTER TABLE job ADD COLUMN hold_code TEXT
    CHECK (hold_code IN ('user', 'quit', 'connector_auth', 'daily_limit', 'storage_limit'));
