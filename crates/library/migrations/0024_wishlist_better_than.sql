-- A better copy the user asked for by hand («Найти качество лучше…»): only
-- an offer above this quality score counts, and a found copy waits for the
-- user's word. NULL for every other item.
ALTER TABLE wishlist ADD COLUMN better_than INTEGER;
