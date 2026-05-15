CREATE TABLE api_keys (
    id INTEGER PRIMARY KEY,
    key_prefix TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    description TEXT NULL,
    expires_at TEXT NULL,
    last_used_at TEXT NULL,
    created_at TEXT NOT NULL
);
