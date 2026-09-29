### Fixed

- **media plugin:** `DbRoomStore::join_room` no longer overshoots the room
  seat cap under concurrent joins (issue #2864). The whole seat claim — room
  row lock, participant recount, cap check, insert, roster reload — now runs
  in one transaction (`FOR NO KEY UPDATE` on the room row on Postgres,
  `BEGIN IMMEDIATE` on SQLite), so racing joiners serialize instead of all
  reading the same pre-join count.
