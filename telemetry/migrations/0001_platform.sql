-- Adds the two payload-v2 columns to a table created before it
-- (`docs/telemetry.md` *The platform*). A database created from the current
-- `schema.sql` already has both and does not need this — SQLite has no
-- `ADD COLUMN IF NOT EXISTS`, so running it there is a "duplicate column
-- name" error and nothing worse.
--
--   npm run db:migrate          (or db:migrate:local)
--
-- Existing rows read '' in both — "did not say" — which is exactly what they
-- did.
ALTER TABLE pings ADD COLUMN distro TEXT NOT NULL DEFAULT '';
ALTER TABLE pings ADD COLUMN os_version TEXT NOT NULL DEFAULT '';
