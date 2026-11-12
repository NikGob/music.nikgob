-- Migration 28 (r35-clip; renumber on merge if another 0028 lands first).
--
-- What a track entry of a source's list is, as the source said it:
-- `song`, `video` or `upload` (YouTube Music's radios hold all three);
-- NULL when the source does not say (and for lists kept before). The junk
-- filter judges a video's title and channel, never an official song's.
ALTER TABLE graph_edge ADD COLUMN item_kind TEXT;
