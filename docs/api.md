# Zeptodo REST API (v1)

All endpoints live under `/api/v1` and require a bearer token:

```
Authorization: Bearer <key>
```

Keys are minted from the web UI (`/api-keys`). The plaintext is shown
exactly once at creation. Lost keys must be revoked and re-created.

## Errors

Every error response uses the same JSON envelope:

```json
{ "error": { "code": "bad_request", "message": "title must not be empty" } }
```

| HTTP | `code`           | Meaning                              |
|------|------------------|--------------------------------------|
| 400  | `bad_request`    | Validation failed or malformed JSON  |
| 401  | `unauthorized`   | Missing, invalid, or expired key     |
| 404  | `not_found`      | No row with that id                  |
| 500  | `internal_error` | Backend failure (see server logs)    |

## Task object

```json
{
  "id": 1,
  "title": "Buy bread",
  "description": "Sourdough",
  "status": "todo",
  "position": 1,
  "created_at": "2026-05-14T12:00:00Z",
  "updated_at": "2026-05-14T12:00:00Z"
}
```

`status` is one of `todo`, `in_progress`, `done`, `cancelled`.

## Endpoints

### `GET /api/v1/tasks`

Query parameters:

- `include_terminal` (bool, default `false`) - include `done` and
  `cancelled` rows.

Response: `200 OK` with `Task[]` ordered by `position ASC`.

### `POST /api/v1/tasks`

Request body:

```json
{ "title": "Buy bread", "description": "Sourdough" }
```

- `title` required, 1 to 128 characters.
- `description` optional, up to 16 KB.

Response: `201 Created` with the persisted `Task`.

### `GET /api/v1/tasks/{id}`

Response: `200 OK` with the `Task`, or `404 not_found`.

### `PUT /api/v1/tasks/{id}`

Same body as create. Response: `200 OK` with the updated `Task`, or
`404 not_found`.

### `DELETE /api/v1/tasks/{id}`

Response: `204 No Content` (idempotent - missing rows still return 204).

### `POST /api/v1/tasks/{id}/status`

Request body:

```json
{ "status": "done" }
```

Response: `200 OK` with the updated `Task`, or `404 not_found`.

### `POST /api/v1/tasks/reorder`

Request body:

```json
{ "ids": [3, 1, 2] }
```

The supplied ids are interpreted as the new visible ordering. Terminal
rows (`done`, `cancelled`) are skipped so they keep their positions.

Response: `200 OK` with:

```json
{ "rewritten": 3 }
```
