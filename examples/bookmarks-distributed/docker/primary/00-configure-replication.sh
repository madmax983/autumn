#!/bin/sh
set -eu

pg_hba="${PGDATA}/pg_hba.conf"
replication_rule="host replication replicator all scram-sha-256"

if ! grep -Fqx "$replication_rule" "$pg_hba"; then
  {
    printf '\n# Allow the standby to stream from the primary.\n'
    printf '%s\n' "$replication_rule"
  } >>"$pg_hba"
fi
