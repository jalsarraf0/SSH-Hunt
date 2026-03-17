CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE TABLE IF NOT EXISTS players (
  id UUID PRIMARY KEY,
  username TEXT NOT NULL,
  display_name TEXT NOT NULL,
  tier TEXT NOT NULL DEFAULT 'noob',
  deaths INTEGER NOT NULL DEFAULT 0,
  banned BOOLEAN NOT NULL DEFAULT FALSE,
  wallet BIGINT NOT NULL DEFAULT 500,
  reputation BIGINT NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_players_username ON players(username);

CREATE TABLE IF NOT EXISTS player_ips (
  id BIGSERIAL PRIMARY KEY,
  player_id UUID NOT NULL REFERENCES players(id) ON DELETE CASCADE,
  remote_ip TEXT NOT NULL,
  seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS player_keys (
  id BIGSERIAL PRIMARY KEY,
  player_id UUID NOT NULL REFERENCES players(id) ON DELETE CASCADE,
  fingerprint TEXT NOT NULL,
  public_key TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (player_id, fingerprint)
);

CREATE TABLE IF NOT EXISTS mission_progress (
  player_id UUID NOT NULL REFERENCES players(id) ON DELETE CASCADE,
  mission_code TEXT NOT NULL,
  completed_at TIMESTAMPTZ,
  PRIMARY KEY (player_id, mission_code)
);

CREATE TABLE IF NOT EXISTS player_settings (
  player_id UUID PRIMARY KEY REFERENCES players(id) ON DELETE CASCADE,
  flash_enabled BOOLEAN NOT NULL DEFAULT TRUE,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
