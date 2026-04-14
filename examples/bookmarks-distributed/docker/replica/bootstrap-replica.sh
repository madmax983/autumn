#!/bin/bash
set -euo pipefail

if [ ! -s "${PGDATA}/PG_VERSION" ]; then
  rm -rf "${PGDATA:?}"/*
  export PGPASSWORD="${REPLICATION_PASSWORD}"

  until pg_isready -h "${PRIMARY_HOST}" -p "${PRIMARY_PORT}" -U "${REPLICATION_USER}"; do
    sleep 1
  done

  pg_basebackup \
    -h "${PRIMARY_HOST}" \
    -p "${PRIMARY_PORT}" \
    -U "${REPLICATION_USER}" \
    -D "${PGDATA}" \
    -Fp \
    -Xs \
    -R \
    -P \
    -S bookmarks_distributed_replica_slot

  chmod 700 "${PGDATA}"
fi

exec docker-entrypoint.sh postgres -c hot_standby=on
