CREATE TABLE score_categories (
  category_key TEXT PRIMARY KEY, rules_version INTEGER NOT NULL,
  display_name TEXT NOT NULL,
  active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0, 1))
);
INSERT INTO score_categories (category_key, rules_version, display_name, active) VALUES
  ('rotation.v1.cluck_hunt', 2, 'Cluck Hunt', 1),
  ('rotation.v1.right_of_way', 2, 'Right of Way', 1);

CREATE TABLE sessions_v2 (
  session_id TEXT PRIMARY KEY, category_key TEXT NOT NULL,
  protocol_version INTEGER NOT NULL, rules_version INTEGER NOT NULL,
  policy_version INTEGER NOT NULL, mode TEXT NOT NULL,
  challenge TEXT NOT NULL, proof TEXT NOT NULL, seed_enc BLOB NOT NULL,
  seed_commitment TEXT NOT NULL CHECK(length(seed_commitment) = 64),
  schedule_hash TEXT NOT NULL CHECK(length(schedule_hash) = 64),
  issued_at INTEGER NOT NULL, start_by_expiry INTEGER,
  started_at INTEGER,
  started INTEGER NOT NULL DEFAULT 0 CHECK(started IN (0, 1)),
  used INTEGER NOT NULL DEFAULT 0 CHECK(used IN (0, 1)),
  turnstile_verified INTEGER NOT NULL DEFAULT 0 CHECK(turnstile_verified IN (0, 1)),
  ip_hash TEXT NOT NULL,
  CHECK((started = 0 AND started_at IS NULL AND start_by_expiry IS NOT NULL)
     OR (started = 1 AND started_at IS NOT NULL AND start_by_expiry IS NULL)),
  FOREIGN KEY(category_key) REFERENCES score_categories(category_key)
);

CREATE TABLE scores_v2 (
  id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL,
  category_key TEXT NOT NULL,
  terminal_total INTEGER NOT NULL CHECK(terminal_total >= 0),
  -- CluckHunt aggregate buckets (NULL for right_of_way)
  chickens INTEGER CHECK(chickens IS NULL OR chickens >= 0),
  coins INTEGER CHECK(coins IS NULL OR coins >= 0),
  -- RightOfWay conduct-specific aggregates (NULL for cluck_hunt)
  signed_accumulator INTEGER,
  premium_bps INTEGER CHECK(premium_bps IS NULL OR (premium_bps >= 0 AND premium_bps <= 10000)),
  packages_delivered INTEGER CHECK(packages_delivered IS NULL OR packages_delivered >= 0),
  courtesy_count INTEGER CHECK(courtesy_count IS NULL OR courtesy_count >= 0),
  animal_hits INTEGER CHECK(animal_hits IS NULL OR animal_hits >= 0),
  objective_completed INTEGER NOT NULL CHECK(objective_completed IN (0, 1)),
  max_combo INTEGER CHECK(max_combo IS NULL OR max_combo BETWEEN 1 AND 5),
  max_delivery_chain INTEGER CHECK(max_delivery_chain IS NULL OR max_delivery_chain >= 0),
  round_duration_ms INTEGER NOT NULL CHECK(round_duration_ms >= 0),
  time_left_ms INTEGER NOT NULL CHECK(time_left_ms >= 0),
  game_over_reason TEXT NOT NULL CHECK(game_over_reason IN ('time_up', 'wrecked')),
  build TEXT NOT NULL,
  platform TEXT NOT NULL CHECK(platform IN ('web', 'native')),
  session_id TEXT NOT NULL UNIQUE, submitted_at INTEGER NOT NULL, ip_hash TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK(status IN ('pending','live','quarantined','unranked_missing_evidence','hidden','deleted')),
  moderation_note TEXT,
  submission_source TEXT NOT NULL DEFAULT 'verified'
    CHECK(submission_source IN ('verified', 'admin_restore')),
  restoration_key TEXT,
  final_root TEXT NOT NULL CHECK(length(final_root) = 64),
  schedule_hash TEXT NOT NULL CHECK(length(schedule_hash) = 64),
  event_count INTEGER NOT NULL CHECK(event_count BETWEEN 1 AND 4096),
  evidence_capability_hash TEXT UNIQUE CHECK(evidence_capability_hash IS NULL OR length(evidence_capability_hash)=64),
  evidence_expires_at INTEGER,
  FOREIGN KEY(category_key) REFERENCES score_categories(category_key),
  FOREIGN KEY(session_id) REFERENCES sessions_v2(session_id),
  CHECK(
    (category_key = 'rotation.v1.cluck_hunt' AND chickens IS NOT NULL AND coins IS NOT NULL
       AND signed_accumulator IS NULL AND max_combo IS NOT NULL
       AND max_delivery_chain IS NULL AND terminal_total = chickens + coins)
    OR
    (category_key = 'rotation.v1.right_of_way' AND signed_accumulator IS NOT NULL
       AND premium_bps IS NOT NULL AND packages_delivered IS NOT NULL
       AND courtesy_count IS NOT NULL AND animal_hits IS NOT NULL
       AND max_delivery_chain IS NOT NULL AND max_combo IS NULL
       AND chickens IS NULL AND coins IS NULL
       AND terminal_total = MAX(0, signed_accumulator))
  )
);

CREATE TABLE score_evidence (
  id INTEGER PRIMARY KEY AUTOINCREMENT, score_id INTEGER NOT NULL UNIQUE,
  session_id TEXT NOT NULL,
  final_root TEXT NOT NULL CHECK(length(final_root) = 64),
  evidence_hash TEXT NOT NULL CHECK(length(evidence_hash) = 64),
  ledger_bytes BLOB NOT NULL CHECK(length(ledger_bytes) BETWEEN 1 AND 262144),
  replay_result TEXT NOT NULL CHECK(replay_result IN ('match','mismatch')),
  quarantine_reason TEXT,
  uploaded_at INTEGER NOT NULL,
  FOREIGN KEY(score_id) REFERENCES scores_v2(id),
  FOREIGN KEY(session_id) REFERENCES sessions_v2(session_id)
);

CREATE TABLE admin_restorations_v2 (
  restoration_key TEXT PRIMARY KEY,
  evidence_hash TEXT NOT NULL UNIQUE CHECK(length(evidence_hash)=64),
  payload_hash TEXT NOT NULL CHECK(length(payload_hash)=64),
  category_key TEXT NOT NULL,
  known_json TEXT NOT NULL CHECK(json_valid(known_json)),
  synthetic_json TEXT NOT NULL CHECK(json_valid(synthetic_json)),
  reason TEXT NOT NULL CHECK(length(reason) BETWEEN 1 AND 256),
  score_id INTEGER NOT NULL UNIQUE,
  restored_at INTEGER NOT NULL,
  admin TEXT NOT NULL,
  FOREIGN KEY(category_key) REFERENCES score_categories(category_key),
  FOREIGN KEY(score_id) REFERENCES scores_v2(id)
);

CREATE TABLE moderation_log_v2 (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  action TEXT NOT NULL CHECK(action IN ('hide','delete','restore')),
  target_score_id INTEGER NOT NULL,
  admin TEXT NOT NULL,
  at INTEGER NOT NULL,
  note TEXT,
  FOREIGN KEY(target_score_id) REFERENCES scores_v2(id)
);

CREATE UNIQUE INDEX idx_scores_v2_restoration_key
  ON scores_v2(restoration_key) WHERE restoration_key IS NOT NULL;
CREATE INDEX idx_sessions_v2_expires_at ON sessions_v2(start_by_expiry);
CREATE INDEX idx_scores_v2_category
  ON scores_v2(category_key, status, terminal_total DESC, submitted_at ASC, id ASC);
CREATE INDEX idx_scores_v2_submitted_at ON scores_v2(submitted_at);
CREATE INDEX idx_scores_v2_pending_evidence ON scores_v2(status,evidence_expires_at) WHERE status='pending';
CREATE INDEX idx_score_evidence_score_root ON score_evidence(score_id,final_root);
