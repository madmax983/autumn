### Fixed

- **autumn-media-plugin:** room join `display_name` values are capped at 64
  characters (`MAX_DISPLAY_NAME_CHARS`); longer names are refused with a
  `400` (`RoomError::DisplayNameTooLong`) before the store admits them (issue
  #3104).
