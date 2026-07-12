-- Add the immutable score id as the terminal leaderboard tie-breaker.
-- Rebuild existing indexes so deployed databases receive the new key.

DROP INDEX IF EXISTS idx_scores_global;
DROP INDEX IF EXISTS idx_scores_condition;

CREATE INDEX idx_scores_global
  ON scores(status, terminal_total DESC, submitted_at ASC, id ASC);

CREATE INDEX idx_scores_condition
  ON scores(condition, status, terminal_total DESC, submitted_at ASC, id ASC);
