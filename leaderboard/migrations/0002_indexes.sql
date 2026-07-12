-- Roady Car leaderboard indexes (migration 0002).
-- Split from 0001 so the schema migration stays readable and index changes
-- can be applied independently in production.

CREATE INDEX idx_scores_global
  ON scores(status, terminal_total DESC, submitted_at ASC);

CREATE INDEX idx_scores_condition
  ON scores(condition, status, terminal_total DESC, submitted_at ASC);

CREATE INDEX idx_scores_submitted_at ON scores(submitted_at);

CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);
