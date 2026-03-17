CREATE TABLE IF NOT EXISTS shop_catalog (
  sku TEXT PRIMARY KEY,
  price BIGINT NOT NULL CHECK (price >= 0),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS auctions (
  id UUID PRIMARY KEY,
  seller_id UUID NOT NULL REFERENCES players(id),
  item_sku TEXT NOT NULL,
  qty INTEGER NOT NULL CHECK (qty > 0),
  start_price BIGINT NOT NULL,
  buyout_price BIGINT,
  highest_bid BIGINT,
  highest_bidder UUID,
  expires_at TIMESTAMPTZ NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  closed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_auctions_open ON auctions(closed_at, expires_at);

CREATE TABLE IF NOT EXISTS chats (
  id UUID PRIMARY KEY,
  channel TEXT NOT NULL,
  sender_display TEXT NOT NULL,
  body TEXT NOT NULL,
  sent_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS parties (
  id UUID PRIMARY KEY,
  leader_id UUID NOT NULL REFERENCES players(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS party_members (
  party_id UUID NOT NULL REFERENCES parties(id) ON DELETE CASCADE,
  player_id UUID NOT NULL REFERENCES players(id) ON DELETE CASCADE,
  joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (party_id, player_id)
);

CREATE TABLE IF NOT EXISTS mails (
  id UUID PRIMARY KEY,
  sender_id UUID REFERENCES players(id),
  recipient_id UUID NOT NULL REFERENCES players(id),
  subject TEXT NOT NULL,
  body TEXT NOT NULL,
  sent_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  read_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS moderation_actions (
  id UUID PRIMARY KEY,
  actor TEXT NOT NULL,
  action TEXT NOT NULL,
  target TEXT NOT NULL,
  reason TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS admin_broadcasts (
  id UUID PRIMARY KEY,
  message TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
