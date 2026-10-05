### Fixed

- **media:** `DbRoomStore::join_room` no longer lets a concurrent `reap_stale`
  sweep silently discard a brand-new seat (issue #2407). The join now runs in
  one transaction and touches the room row (`SET created_at = created_at`)
  before inserting the participant: a reaper `DELETE` blocked on that lock
  wakes to a new row version, so its empty-room check is re-evaluated and the
  room is skipped. If the reaper commits first, the join fails loud with
  `RoomNotFound` instead of a phantom success.
