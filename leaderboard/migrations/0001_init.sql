-- Roady Car leaderboard D1 schema (migration 0001).
-- See LEADERBOARD_ARCHITECTURE.md §3 for the authoritative schema.

CREATE TABLE sessions (
  session_id TEXT PRIMARY KEY,
  challenge TEXT NOT NULL,
  condition INTEGER NOT NULL CHECK(condition BETWEEN 0 AND 4),
  proof TEXT NOT NULL,
  issued_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  used INTEGER NOT NULL DEFAULT 0 CHECK(used IN (0, 1)),
  turnstile_verified INTEGER NOT NULL DEFAULT 0 CHECK(turnstile_verified IN (0, 1)),
  ip_hash TEXT NOT NULL
);

CREATE TABLE scores (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  condition INTEGER NOT NULL CHECK(condition BETWEEN 0 AND 4),
  terminal_total INTEGER NOT NULL CHECK(terminal_total >= 0),
  chickens INTEGER NOT NULL CHECK(chickens >= 0),
  coins INTEGER NOT NULL CHECK(coins >= 0),
  objective_completed INTEGER NOT NULL CHECK(objective_completed IN (0, 1)),
  max_combo INTEGER NOT NULL CHECK(max_combo BETWEEN 1 AND 5),
  round_duration_ms INTEGER NOT NULL CHECK(round_duration_ms >= 0),
  time_left_ms INTEGER NOT NULL CHECK(time_left_ms >= 0),
  game_over_reason TEXT NOT NULL CHECK(game_over_reason IN ('time_up', 'wrecked')),
  build TEXT NOT NULL,
  platform TEXT NOT NULL CHECK(platform IN ('web', 'native')),
  session_id TEXT NOT NULL UNIQUE,
  submitted_at INTEGER NOT NULL,
  ip_hash TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'live' CHECK(status IN ('live', 'hidden', 'deleted')),
  moderation_note TEXT,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id)
);

CREATE TABLE moderation_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  action TEXT NOT NULL,
  target_score_id INTEGER NOT NULL,
  admin TEXT NOT NULL,
  at INTEGER NOT NULL,
  note TEXT
);
