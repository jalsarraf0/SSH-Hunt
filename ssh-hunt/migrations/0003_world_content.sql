CREATE TABLE IF NOT EXISTS lore_entries (
  code TEXT PRIMARY KEY,
  body TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS world_events (
  id UUID PRIMARY KEY,
  sector TEXT NOT NULL,
  title TEXT NOT NULL,
  starts_at TIMESTAMPTZ NOT NULL,
  ends_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS script_assets (
  id UUID PRIMARY KEY,
  owner_id UUID REFERENCES players(id),
  name TEXT NOT NULL,
  source TEXT NOT NULL,
  is_marketplace BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS script_runs (
  id UUID PRIMARY KEY,
  player_id UUID NOT NULL REFERENCES players(id),
  script_name TEXT NOT NULL,
  exit_code INTEGER NOT NULL,
  consumed_ops BIGINT NOT NULL,
  elapsed_ms BIGINT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS achievements (
  player_id UUID NOT NULL REFERENCES players(id),
  code TEXT NOT NULL,
  unlocked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (player_id, code)
);
