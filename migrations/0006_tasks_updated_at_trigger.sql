CREATE TRIGGER tasks_set_updated_at
AFTER UPDATE OF title, description, status ON tasks
BEGIN
    UPDATE tasks
    SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE id = NEW.id;
END;
