CREATE TABLE IF NOT EXISTS vfs_content (
  hash TEXT PRIMARY KEY,
  body TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS vfs_nodes (
  id UUID PRIMARY KEY,
  player_id UUID NOT NULL REFERENCES players(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  kind TEXT NOT NULL,
  owner TEXT NOT NULL,
  perms INTEGER NOT NULL,
  content_hash TEXT REFERENCES vfs_content(hash),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(player_id, path)
);
