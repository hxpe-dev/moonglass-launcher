-- Initial schema (spec section 6). `PRAGMA foreign_keys = ON` is set per
-- connection by moonglass-db, because SQLite does not persist it.

CREATE TABLE emulators (
    id               TEXT PRIMARY KEY NOT NULL,
    name             TEXT NOT NULL,            -- "Dolphin", "PCSX2", "RetroArch"
    executable_path  TEXT NOT NULL,
    launch_template  TEXT NOT NULL,            -- arguments only, e.g. -b -e "{rom}"
    file_extensions  TEXT NOT NULL,            -- ".iso,.rvz,.gcm"
    folder_marker    TEXT                      -- e.g. "EBOOT.BIN" for folder-based games
);

CREATE TABLE rom_directories (
    id           TEXT PRIMARY KEY NOT NULL,
    path         TEXT NOT NULL UNIQUE,
    emulator_id  TEXT NOT NULL REFERENCES emulators(id) ON DELETE CASCADE,
    platform     TEXT NOT NULL,                -- display label, e.g. "GameCube"
    recursive    INTEGER NOT NULL DEFAULT 1 CHECK (recursive IN (0, 1))
);

CREATE TABLE games (
    id                TEXT PRIMARY KEY NOT NULL, -- see ID rules
    title             TEXT NOT NULL,
    sort_title        TEXT NOT NULL,           -- lowercased, leading articles removed
    search_title      TEXT NOT NULL,           -- lowercased, accents and punctuation stripped
    source            TEXT NOT NULL CHECK (source IN ('steam', 'epic', 'exe', 'rom')),
    source_id         TEXT,                    -- Steam AppID or Epic AppName; NULL for exe/rom
    launch_target     TEXT,                    -- .exe path or ROM path; NULL for steam/epic
    launch_args       TEXT,
    working_dir       TEXT,                    -- exe only; NULL = executable's folder
    rom_directory_id  TEXT REFERENCES rom_directories(id) ON DELETE CASCADE,
    platform          TEXT,                    -- copied from the ROM directory, for badges
    install_dir       TEXT,                    -- absolute path; watched for steam/epic
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

CREATE INDEX idx_games_order ON games (is_installed, is_hidden, last_played DESC, date_added DESC);

CREATE TABLE settings (
    key    TEXT PRIMARY KEY NOT NULL,
    value  TEXT NOT NULL                       -- JSON-encoded
);
