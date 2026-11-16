-- Migration 34.
--
-- The owner's paired devices («Межсеть»): `node` (unused until now, `id` is
-- the device's node id) gets what the devices list keeps between runs.
-- `name` stays the device's own name (its certificate's, then what it says
-- of itself); `alias` is a name given here, over it. `scope` is 'owner' or
-- 'guest'; `profile` what the device stores ('full', 'library',
-- 'favorites:<bytes>', 'sound'); `addr` the `ip:port` it was last reached
-- at; `version` the player it runs; `cert` its ring certificate (JSON);
-- `paired_at` when it joined (unix ms).
ALTER TABLE node ADD COLUMN alias TEXT;
ALTER TABLE node ADD COLUMN scope TEXT NOT NULL DEFAULT 'owner';
ALTER TABLE node ADD COLUMN profile TEXT NOT NULL DEFAULT 'full';
ALTER TABLE node ADD COLUMN addr TEXT;
ALTER TABLE node ADD COLUMN version TEXT;
ALTER TABLE node ADD COLUMN cert TEXT;
ALTER TABLE node ADD COLUMN paired_at INTEGER;
