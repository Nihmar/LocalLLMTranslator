-- Persisted application state: a tiny key/value table for flags that must
-- survive a relaunch. The first user is the explicit worker-pool pause
-- (`worker_paused`), so a pause is not silently undone by boot auto-resume.
-- Values are stored as text; the control plane owns their interpretation.

CREATE TABLE app_state (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
