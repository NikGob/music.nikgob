-- Migration 31.
--
-- Where a lossless master laid out by name lives: new masters go to
-- `<masters>/<Album artist>/<YYYY - Album>/<NN - Title>.flac` and the
-- library keeps that full path. NULL for every master there is now: they
-- stay `<masters>/{file_hash}.<ext>` (or where their torrent keeps them)
-- and are found as before. Only a column is added: no row changes.
ALTER TABLE master ADD COLUMN file_path TEXT;
