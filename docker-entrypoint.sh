#!/bin/sh
set -e

echo "Starting Zeptodo..."
echo "Bind address:  ${BIND_ADDR}"
echo "Database URL:  ${DATABASE_URL}"
echo "Base URL:      ${BASE_URL}"
echo "Timezone:      ${TIMEZONE}"
echo "Log directory: ${LOG_DIR:-<stdout only>}"

if [ ! -d /data ]; then
    echo "ERROR: /data directory does not exist. Mount a volume there." >&2
    exit 1
fi

if [ ! -w /data ]; then
    echo "ERROR: /data is not writable. Check volume ownership (uid/gid 1000)." >&2
    exit 1
fi

if [ -n "${LOG_DIR}" ]; then
    mkdir -p "${LOG_DIR}"
fi

case "${DATABASE_URL}" in
    sqlite:*)
        db_path="${DATABASE_URL#sqlite:}"
        db_path="${db_path#//}"
        case "${db_path}" in
            /*) db_dir=$(dirname "${db_path}") ;;
            *)  db_dir=$(dirname "/app/${db_path}") ;;
        esac
        mkdir -p "${db_dir}"
        ;;
esac

exec "$@"
