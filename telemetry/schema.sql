-- The Alter Zero telemetry table (docs/telemetry.md): one row per install per
-- UTC day, whatever the client does — the primary key is the dedup, and the
-- worker's `ON CONFLICT(day, id) DO UPDATE` refreshes the row on the day's
-- second ping (the one an update sends), so it says what the install is on now.
--
--   npx wrangler d1 execute alter-zero-telemetry --remote --file=schema.sql
CREATE TABLE IF NOT EXISTS pings (
  day     TEXT NOT NULL,  -- the UTC date the ping ARRIVED (YYYY-MM-DD), by the server's clock
  id      TEXT NOT NULL,  -- the install's anonymous id: 32 lowercase hex characters
  country TEXT NOT NULL,  -- ISO 3166-1 alpha-2 from the edge (request.cf.country); 'ZZ' when it had none
  version TEXT NOT NULL,  -- the app version the ping named
  os      TEXT NOT NULL,  -- std::env::consts::OS
  arch    TEXT NOT NULL,  -- std::env::consts::ARCH
  distro  TEXT NOT NULL DEFAULT '',  -- the Linux distribution's os-release ID; '' off Linux, and from any client older than payload v2
  os_version TEXT NOT NULL DEFAULT '',  -- the platform's own version: the distribution's VERSION_ID, or macOS's ProductVersion; '' when it names none
  PRIMARY KEY (day, id)
);

-- "New installs" and "installs seen" walk the ids; the primary key already
-- serves every per-day query.
CREATE INDEX IF NOT EXISTS pings_by_id ON pings (id, day);
