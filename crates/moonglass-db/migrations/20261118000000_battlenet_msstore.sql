-- Battle.net and Microsoft Store games (spec 2): adds 'battlenet' and
-- 'msstore' to the allowed sources. As for GOG, SQLite cannot change a
-- CHECK constraint in place, so the table is rebuilt.

CREATE TABLE games_new (
    id                TEXT PRIMARY KEY NOT NULL, -- see ID rules
    title             TEXT NOT NULL,
    sort_title        TEXT NOT NULL,           -- lowercased, leading articles removed
    search_title      TEXT NOT NULL,           -- lowercased, accents and punctuation stripped
    source            TEXT NOT NULL CHECK (source IN ('steam', 'epic', 'gog', 'battlenet', 'msstore', 'exe', 'rom')),
    source_id         TEXT,                    -- Steam AppID, Epic AppName, GOG product ID, Battle.net UID or Store app ID; NULL for exe/rom
    launch_target     TEXT,                    -- .exe or ROM path; for GOG and Battle.net, the game's own program; NULL for steam/epic/msstore
    launch_args       TEXT,
    working_dir       TEXT,                    -- exe only; NULL = executable's folder
    rom_directory_id  TEXT REFERENCES rom_directories(id) ON DELETE CASCADE,
    platform          TEXT,                    -- copied from the ROM directory, for badges
    install_dir       TEXT,                    -- absolute path; watched for store games
    watch_process     TEXT,                    -- optional exact process name override
    track_by_folder   INTEGER NOT NULL DEFAULT 0 CHECK (track_by_folder IN (0, 1)),

    cover_path        TEXT,
    hero_path         TEXT,
    logo_path         TEXT,
    dominant_color    TEXT,                    -- "#RRGGBB"
    accent_color      TEXT,                    -- "#RRGGBB"
    artwork_locked    INTEGER NOT NULL DEFAULT 0 CHECK (artwork_locked IN (0, 1)),

    date_added        INTEGER NOT NULL,        -- Unix seconds
    last_played       INTEGER,                 -- Unix seconds; NULL = never
    playtime_seconds  INTEGER NOT NULL DEFAULT 0,   -- sessions launched from Moonglass only
    store_playtime_seconds INTEGER NOT NULL DEFAULT 0, -- what the store reports
    is_favorite       INTEGER NOT NULL DEFAULT 0 CHECK (is_favorite IN (0, 1)),
    is_hidden         INTEGER NOT NULL DEFAULT 0 CHECK (is_hidden IN (0, 1)),
    is_installed      INTEGER NOT NULL DEFAULT 1 CHECK (is_installed IN (0, 1)),

    UNIQUE (source, source_id)
);

INSERT INTO games_new (
    id, title, sort_title, search_title, source, source_id, launch_target, launch_args,
    working_dir, rom_directory_id, platform, install_dir, watch_process, track_by_folder,
    cover_path, hero_path, logo_path, dominant_color, accent_color, artwork_locked,
    date_added, last_played, playtime_seconds, store_playtime_seconds, is_favorite, is_hidden, is_installed)
SELECT
    id, title, sort_title, search_title, source, source_id, launch_target, launch_args,
    working_dir, rom_directory_id, platform, install_dir, watch_process, track_by_folder,
    cover_path, hero_path, logo_path, dominant_color, accent_color, artwork_locked,
    date_added, last_played, playtime_seconds, store_playtime_seconds, is_favorite, is_hidden, is_installed
FROM games;

DROP TABLE games;
ALTER TABLE games_new RENAME TO games;

CREATE INDEX idx_games_order ON games (is_installed, is_hidden, last_played DESC, date_added DESC);
