-- GOG games (spec 2): adds 'gog' to the allowed sources. SQLite cannot
-- change a CHECK constraint in place, so the table is rebuilt. Nothing
-- references `games`, so dropping the old table is safe.

CREATE TABLE games_new (
    id                TEXT PRIMARY KEY NOT NULL, -- see ID rules
    title             TEXT NOT NULL,
    sort_title        TEXT NOT NULL,           -- lowercased, leading articles removed
    search_title      TEXT NOT NULL,           -- lowercased, accents and punctuation stripped
    source            TEXT NOT NULL CHECK (source IN ('steam', 'epic', 'gog', 'exe', 'rom')),
    source_id         TEXT,                    -- Steam AppID, Epic AppName or GOG product ID; NULL for exe/rom
    launch_target     TEXT,                    -- .exe or ROM path; for GOG, the game's own executable; NULL for steam/epic
    launch_args       TEXT,
    working_dir       TEXT,                    -- exe only; NULL = executable's folder
    rom_directory_id  TEXT REFERENCES rom_directories(id) ON DELETE CASCADE,
    platform          TEXT,                    -- copied from the ROM directory, for badges
    install_dir       TEXT,                    -- absolute path; watched for steam/epic/gog
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
    is_favorite       INTEGER NOT NULL DEFAULT 0 CHECK (is_favorite IN (0, 1)),
    is_hidden         INTEGER NOT NULL DEFAULT 0 CHECK (is_hidden IN (0, 1)),
    is_installed      INTEGER NOT NULL DEFAULT 1 CHECK (is_installed IN (0, 1)),

    UNIQUE (source, source_id)
);

INSERT INTO games_new (
    id, title, sort_title, search_title, source, source_id, launch_target, launch_args,
    working_dir, rom_directory_id, platform, install_dir, watch_process, track_by_folder,
    cover_path, hero_path, logo_path, dominant_color, accent_color, artwork_locked,
    date_added, last_played, playtime_seconds, is_favorite, is_hidden, is_installed)
SELECT
    id, title, sort_title, search_title, source, source_id, launch_target, launch_args,
    working_dir, rom_directory_id, platform, install_dir, watch_process, track_by_folder,
    cover_path, hero_path, logo_path, dominant_color, accent_color, artwork_locked,
    date_added, last_played, playtime_seconds, is_favorite, is_hidden, is_installed
FROM games;

DROP TABLE games;
ALTER TABLE games_new RENAME TO games;

CREATE INDEX idx_games_order ON games (is_installed, is_hidden, last_played DESC, date_added DESC);
