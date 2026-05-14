CREATE TABLE credentials (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    username TEXT NOT NULL CHECK (length(username) <= 64),
    password_hash TEXT NOT NULL,
    timezone TEXT NOT NULL,
    last_login_at TEXT NULL
);
