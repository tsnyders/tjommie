CREATE TABLE routine_failure_notifications (
    run_id TEXT PRIMARY KEY REFERENCES routine_runs(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    body TEXT NOT NULL
  );