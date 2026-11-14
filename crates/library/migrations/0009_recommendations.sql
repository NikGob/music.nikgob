-- Recommendations (stage 4): where a sound analysis came
-- from, files that cannot be analysed, and "do not recommend" feedback.

-- analysis.source: what the vector was computed from. A file's analysis
-- always wins; a stream's (the track's song on YouTube Music, for a track
-- without a file) is computed again once a file arrives. Every existing
-- vector came from a file.
ALTER TABLE analysis ADD COLUMN source TEXT NOT NULL DEFAULT 'file';   -- file | youtube

-- A local file the analysis could not decode, keyed by the file it was
-- (source id, hash and size): not tried again until the track's file
-- changes. Derived data; goes with the track.
CREATE TABLE analysis_failed (
    track_id  TEXT PRIMARY KEY,
    file_key  TEXT NOT NULL,
    error     TEXT NOT NULL,
    failed_at INTEGER NOT NULL
) WITHOUT ROWID;

-- «Не рекомендовать трек / исполнителя»: last writer wins per target, like
-- `likes`; a cleared dislike stays as dislike = 0 with its time.
CREATE TABLE rec_feedback (
    target_kind TEXT NOT NULL,                -- track | artist (and what the recommender adds)
    target_id   TEXT NOT NULL,
    dislike     INTEGER NOT NULL DEFAULT 1,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (target_kind, target_id)
) WITHOUT ROWID;
CREATE INDEX rec_feedback_disliked ON rec_feedback(target_kind) WHERE dislike = 1;

-- The analysis queue and progress walk the tracks with a file.
CREATE INDEX track_local ON track(has_local) WHERE has_local = 1;
