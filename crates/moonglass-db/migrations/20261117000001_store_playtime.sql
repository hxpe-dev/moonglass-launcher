-- Playtime the game's store reports (Steam's own count for the signed-in
-- account), kept apart from `playtime_seconds`, which counts only sessions
-- started from Moonglass. The store's figure already includes those
-- sessions, so the two are never added: the larger one is shown.
ALTER TABLE games ADD COLUMN store_playtime_seconds INTEGER NOT NULL DEFAULT 0;
