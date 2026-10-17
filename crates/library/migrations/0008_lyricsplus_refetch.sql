-- LyricsPlus texts cached before the provider was read: QQ Music answers
-- word timing generated for non-Chinese songs (and credit lines), and a
-- stored body cannot tell its provider. They are fetched again; a pick of
-- LyricsPlus goes with them. Only the lyrics cache is touched.
DELETE FROM lyrics_text WHERE source = 'lyricsplus';
DELETE FROM lyrics_pick WHERE source = 'lyricsplus';
