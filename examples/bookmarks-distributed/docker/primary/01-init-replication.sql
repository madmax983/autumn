CREATE ROLE replicator WITH REPLICATION LOGIN PASSWORD 'replicator';
SELECT slot_name
FROM pg_create_physical_replication_slot('bookmarks_distributed_replica_slot');
