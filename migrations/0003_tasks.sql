CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL CHECK (length(title) <= 128),
    description TEXT NULL CHECK (description IS NULL OR length(description) <= 16384),
    status TEXT NOT NULL CHECK (status IN ('todo', 'in_progress', 'done', 'cancelled')),
    position INTEGER NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_tasks_position ON tasks (position);
CREATE INDEX idx_tasks_status ON tasks (status);
