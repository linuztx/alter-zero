-- The Alter Zero telemetry table (docs/telemetry.md): one row per install per
-- UTC day, whatever the client does — the primary key is the dedup.
--
--   npx wrangler d1 execute alter-zero-telemetry --remote --file=schema.sql
CREATE TABLE IF NOT EXISTS pings (
  day     TEXT NOT NULL,  -- the UTC date the ping ARRIVED (YYYY-MM-DD), by the server's clock
  id      TEXT NOT NULL,  -- the install's anonymous id: 32 lowercase hex characters
  country TEXT NOT NULL,  -- ISO 3166-1 alpha-2 from the edge (request.cf.country); 'ZZ' when it had none
  version TEXT NOT NULL,  -- the app version the ping named
  os      TEXT NOT NULL,  -- std::env::consts::OS
  arch    TEXT NOT NULL,  -- std::env::consts::ARCH
  PRIMARY KEY (day, id)
);

-- "New installs" and "installs seen" walk the ids; the primary key already
-- serves every per-day query.
CREATE INDEX IF NOT EXISTS pings_by_id ON pings (id, day);
