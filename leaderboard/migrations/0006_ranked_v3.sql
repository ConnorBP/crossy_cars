-- Additive rules-v3 storage. These tables deliberately have no references to
-- v1/v2 tables so a route can never cross a board epoch by accident.
CREATE TABLE score_categories_v3 (
  category_key TEXT PRIMARY KEY,
  protocol_version INTEGER NOT NULL CHECK(protocol_version = 3),
  protocol_id TEXT NOT NULL CHECK(protocol_id = 'roady-protocol.v3'),
  rules_version INTEGER NOT NULL CHECK(rules_version = 3),
  rules_id TEXT NOT NULL CHECK(rules_id = 'roady-rules.v3'),
  policy_version INTEGER NOT NULL CHECK(policy_version = 1),
  policy_id TEXT NOT NULL CHECK(policy_id = 'roady-ranked-policy.v3.1'),
  mode TEXT NOT NULL CHECK(mode = 'rotation'),
  conduct TEXT NOT NULL CHECK(conduct IN ('cluck_hunt','right_of_way')),
  display_name TEXT NOT NULL,
  active INTEGER NOT NULL CHECK(active = 1),
  CHECK((category_key='rotation.v2.cluck_hunt' AND conduct='cluck_hunt' AND display_name='Cluck Hunt') OR
        (category_key='rotation.v2.right_of_way' AND conduct='right_of_way' AND display_name='Right of Way')),
  UNIQUE(category_key, protocol_version, protocol_id, rules_version, rules_id,
         policy_version, policy_id, mode, conduct)
);
INSERT INTO score_categories_v3 VALUES
 ('rotation.v2.cluck_hunt',3,'roady-protocol.v3',3,'roady-rules.v3',1,'roady-ranked-policy.v3.1','rotation','cluck_hunt','Cluck Hunt',1),
 ('rotation.v2.right_of_way',3,'roady-protocol.v3',3,'roady-rules.v3',1,'roady-ranked-policy.v3.1','rotation','right_of_way','Right of Way',1);

CREATE TABLE sessions_v3 (
  session_id TEXT PRIMARY KEY CHECK(length(CAST(session_id AS BLOB)) BETWEEN 1 AND 255),
  category_key TEXT NOT NULL,
  protocol_version INTEGER NOT NULL CHECK(protocol_version=3), protocol_id TEXT NOT NULL CHECK(protocol_id='roady-protocol.v3'),
  rules_version INTEGER NOT NULL CHECK(rules_version=3), rules_id TEXT NOT NULL CHECK(rules_id='roady-rules.v3'),
  policy_version INTEGER NOT NULL CHECK(policy_version=1), policy_id TEXT NOT NULL CHECK(policy_id='roady-ranked-policy.v3.1'),
  mode TEXT NOT NULL CHECK(mode='rotation'), conduct TEXT NOT NULL CHECK(conduct IN ('cluck_hunt','right_of_way')),
  challenge TEXT NOT NULL CHECK(length(CAST(challenge AS BLOB)) BETWEEN 1 AND 255),
  proof TEXT NOT NULL,
  seed_iv BLOB NOT NULL CHECK(length(seed_iv)=12),
  seed_ciphertext BLOB NOT NULL CHECK(length(seed_ciphertext)=48),
  seed_key_id TEXT NOT NULL CHECK(length(seed_key_id) BETWEEN 1 AND 64 AND seed_key_id GLOB 'v3.seed.*'),
  seed_commitment TEXT NOT NULL CHECK(length(seed_commitment)=64 AND seed_commitment GLOB '[0-9a-f]*'),
  schedule_hash TEXT NOT NULL CHECK(length(schedule_hash)=64 AND schedule_hash GLOB '[0-9a-f]*'),
  issued_at INTEGER NOT NULL CHECK(issued_at>=0), start_by_expiry INTEGER, started_at INTEGER,
  started INTEGER NOT NULL DEFAULT 0 CHECK(started IN (0,1)), used INTEGER NOT NULL DEFAULT 0 CHECK(used IN (0,1)),
  turnstile_verified INTEGER NOT NULL CHECK(turnstile_verified IN (0,1)), ip_hash TEXT NOT NULL,
  CHECK((started=0 AND used=0 AND started_at IS NULL AND start_by_expiry=issued_at+300000) OR
        (started=1 AND started_at IS NOT NULL AND start_by_expiry IS NULL)),
  CHECK((turnstile_verified=1) OR (session_id LIKE 'admin_restore:%' AND started=1 AND used=1)),
  FOREIGN KEY(category_key,protocol_version,protocol_id,rules_version,rules_id,policy_version,policy_id,mode,conduct)
    REFERENCES score_categories_v3(category_key,protocol_version,protocol_id,rules_version,rules_id,policy_version,policy_id,mode,conduct)
);

CREATE TABLE scores_v3 (
  id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL CHECK(name GLOB '[A-Z0-9][A-Z0-9][A-Z0-9]*' AND length(name) BETWEEN 3 AND 5),
  category_key TEXT NOT NULL,
  protocol_version INTEGER NOT NULL CHECK(protocol_version=3), protocol_id TEXT NOT NULL CHECK(protocol_id='roady-protocol.v3'),
  rules_version INTEGER NOT NULL CHECK(rules_version=3), rules_id TEXT NOT NULL CHECK(rules_id='roady-rules.v3'),
  policy_version INTEGER NOT NULL CHECK(policy_version=1), policy_id TEXT NOT NULL CHECK(policy_id='roady-ranked-policy.v3.1'),
  mode TEXT NOT NULL CHECK(mode='rotation'), conduct TEXT NOT NULL CHECK(conduct IN ('cluck_hunt','right_of_way')),
  terminal_total INTEGER NOT NULL CHECK(terminal_total BETWEEN 0 AND 4294967295),
  chickens INTEGER, coins INTEGER, signed_accumulator INTEGER, premium_bps INTEGER,
  packages_delivered INTEGER, courtesy_count INTEGER, animal_hits INTEGER,
  objective_completed INTEGER NOT NULL CHECK(objective_completed IN (0,1)), max_combo INTEGER, max_delivery_chain INTEGER,
  round_duration_ms INTEGER NOT NULL CHECK(round_duration_ms>=0), time_left_ms INTEGER NOT NULL CHECK(time_left_ms BETWEEN 0 AND 99000),
  game_over_reason TEXT NOT NULL CHECK(game_over_reason IN ('time_up','wrecked','drowned')),
  build TEXT NOT NULL CHECK(length(CAST(build AS BLOB)) BETWEEN 1 AND 64), platform TEXT NOT NULL CHECK(platform IN ('web','native')),
  session_id TEXT NOT NULL UNIQUE, submitted_at INTEGER NOT NULL CHECK(submitted_at>=0), ip_hash TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','live','quarantined','unranked_missing_evidence','hidden','deleted')),
  moderation_note TEXT, submission_source TEXT NOT NULL CHECK(submission_source IN ('verified','admin_restore')), restoration_key TEXT,
  final_root TEXT NOT NULL CHECK(length(final_root)=64 AND final_root NOT GLOB '*[^0-9a-f]*'),
  schedule_hash TEXT NOT NULL CHECK(length(schedule_hash)=64 AND schedule_hash NOT GLOB '*[^0-9a-f]*'),
  seed_commitment TEXT NOT NULL CHECK(length(seed_commitment)=64 AND seed_commitment NOT GLOB '*[^0-9a-f]*'),
  event_count INTEGER NOT NULL CHECK(event_count BETWEEN 1 AND 4096),
  signature_key_id TEXT NOT NULL CHECK(length(signature_key_id) BETWEEN 1 AND 64 AND (signature_key_id GLOB 'v3.client.*' OR signature_key_id='admin_restore')),
  evidence_capability_hash TEXT UNIQUE CHECK(evidence_capability_hash IS NULL OR length(evidence_capability_hash)=64), evidence_expires_at INTEGER,
  CHECK((submission_source='verified' AND status='pending' AND evidence_capability_hash IS NOT NULL AND evidence_expires_at=submitted_at+86400000 AND restoration_key IS NULL) OR
        (submission_source='admin_restore' AND status='live' AND evidence_capability_hash IS NULL AND evidence_expires_at IS NULL AND restoration_key IS NOT NULL) OR
        (submission_source='verified' AND status<>'pending' AND evidence_capability_hash IS NOT NULL AND evidence_expires_at IS NOT NULL AND restoration_key IS NULL)),
  CHECK((conduct='cluck_hunt' AND category_key='rotation.v2.cluck_hunt' AND chickens>=0 AND coins>=0 AND terminal_total=chickens+coins
         AND signed_accumulator IS NULL AND premium_bps IS NULL AND packages_delivered IS NULL AND courtesy_count IS NULL AND animal_hits IS NULL
         AND max_combo BETWEEN 1 AND 5 AND max_delivery_chain IS NULL) OR
        (conduct='right_of_way' AND category_key='rotation.v2.right_of_way' AND chickens IS NULL AND coins IS NULL AND signed_accumulator IS NOT NULL
         AND premium_bps BETWEEN 0 AND 10000 AND packages_delivered>=0 AND courtesy_count>=0 AND animal_hits>=0 AND max_combo IS NULL
         AND max_delivery_chain>=0 AND terminal_total=MAX(0,signed_accumulator))),
  FOREIGN KEY(category_key,protocol_version,protocol_id,rules_version,rules_id,policy_version,policy_id,mode,conduct)
    REFERENCES score_categories_v3(category_key,protocol_version,protocol_id,rules_version,rules_id,policy_version,policy_id,mode,conduct),
  FOREIGN KEY(session_id) REFERENCES sessions_v3(session_id)
);

CREATE TABLE score_evidence_v3 (
 id INTEGER PRIMARY KEY AUTOINCREMENT, score_id INTEGER NOT NULL UNIQUE, session_id TEXT NOT NULL,
 final_root TEXT NOT NULL CHECK(length(final_root)=64 AND final_root NOT GLOB '*[^0-9a-f]*'),
 evidence_hash TEXT NOT NULL CHECK(length(evidence_hash)=64 AND evidence_hash NOT GLOB '*[^0-9a-f]*'),
 ledger_bytes BLOB NOT NULL CHECK(length(ledger_bytes) BETWEEN 1 AND 262144),
 replay_result TEXT NOT NULL CHECK(replay_result IN ('match','mismatch')), quarantine_reason TEXT, uploaded_at INTEGER NOT NULL,
 CHECK((replay_result='match' AND quarantine_reason IS NULL) OR (replay_result='mismatch' AND quarantine_reason IS NOT NULL)),
 FOREIGN KEY(score_id) REFERENCES scores_v3(id), FOREIGN KEY(session_id) REFERENCES sessions_v3(session_id)
);
CREATE TABLE admin_restorations_v3 (
 restoration_key TEXT PRIMARY KEY, evidence_hash TEXT NOT NULL UNIQUE CHECK(length(evidence_hash)=64), payload_hash TEXT NOT NULL CHECK(length(payload_hash)=64),
 category_key TEXT NOT NULL, known_json TEXT NOT NULL CHECK(json_valid(known_json)), synthetic_json TEXT NOT NULL CHECK(json_valid(synthetic_json)),
 reason TEXT NOT NULL CHECK(length(CAST(reason AS BLOB)) BETWEEN 1 AND 256), score_id INTEGER NOT NULL UNIQUE, restored_at INTEGER NOT NULL, admin TEXT NOT NULL,
 FOREIGN KEY(category_key) REFERENCES score_categories_v3(category_key), FOREIGN KEY(score_id) REFERENCES scores_v3(id)
);
CREATE TABLE moderation_log_v3 (
 id INTEGER PRIMARY KEY AUTOINCREMENT, action TEXT NOT NULL CHECK(action IN ('hide','delete','restore')), target_score_id INTEGER NOT NULL,
 admin TEXT NOT NULL, at INTEGER NOT NULL, note TEXT, FOREIGN KEY(target_score_id) REFERENCES scores_v3(id)
);
CREATE UNIQUE INDEX idx_scores_v3_restoration_key ON scores_v3(restoration_key) WHERE restoration_key IS NOT NULL;
CREATE INDEX idx_sessions_v3_expiry ON sessions_v3(start_by_expiry) WHERE started=0;
CREATE INDEX idx_scores_v3_category ON scores_v3(category_key,status,terminal_total DESC,submitted_at ASC,id ASC);
CREATE INDEX idx_scores_v3_pending_evidence ON scores_v3(status,evidence_expires_at) WHERE status='pending';
CREATE INDEX idx_score_evidence_v3_root ON score_evidence_v3(score_id,final_root);
