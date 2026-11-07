-- How a play was heard (profiles-v1 §11): the app's window and the
-- player's volume while it played, on the end of a play. Older rows and
-- imported plays keep NULL.

ALTER TABLE event ADD COLUMN attention REAL;     -- 0 focused, 0.5 in view behind another window, 1 hidden or minimised
ALTER TABLE event ADD COLUMN volume REAL;        -- 0..1, the player's mean volume
