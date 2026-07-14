-- Evidence-backed, idempotent administrator score restoration.
-- A restored score is distinguishable from a normal verified submission and
-- is linked one-to-one to its audit record by restoration_key.

ALTER TABLE scores ADD COLUMN submission_source TEXT NOT NULL DEFAULT 'verified'
  CHECK(submission_source IN ('verified', 'admin_restore'));
ALTER TABLE scores ADD COLUMN restoration_key TEXT;

CREATE UNIQUE INDEX idx_scores_restoration_key
  ON scores(restoration_key)
  WHERE restoration_key IS NOT NULL;

CREATE TABLE admin_restorations (
  restoration_key TEXT PRIMARY KEY,
  evidence_hash TEXT NOT NULL UNIQUE CHECK(length(evidence_hash) = 64),
  payload_hash TEXT NOT NULL CHECK(length(payload_hash) = 64),
  known_fields_json TEXT NOT NULL CHECK(json_valid(known_fields_json) AND length(known_fields_json) <= 4096),
  synthetic_fields_json TEXT NOT NULL CHECK(json_valid(synthetic_fields_json) AND length(synthetic_fields_json) <= 4096),
  reason TEXT NOT NULL CHECK(length(reason) BETWEEN 1 AND 256),
  score_id INTEGER NOT NULL UNIQUE,
  restored_at INTEGER NOT NULL,
  admin TEXT NOT NULL,
  FOREIGN KEY(score_id) REFERENCES scores(id)
);
