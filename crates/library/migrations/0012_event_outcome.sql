-- How a play came and went (recs-v2 §15, phase 2): who chose it, what
-- played it and why it ended. Older rows keep NULL.

ALTER TABLE event ADD COLUMN origin TEXT;        -- manual | queue | autoplay | radio | mix
ALTER TABLE event ADD COLUMN context_id TEXT;    -- release / playlist / mix key / radio seed
ALTER TABLE event ADD COLUMN end_reason TEXT;    -- user_next | user_pick | app_closed | ended
