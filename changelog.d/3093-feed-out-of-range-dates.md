### Fixed

- **feed:** out-of-range entry/channel dates no longer panic RSS rendering or
  emit invalid Atom dates — years outside 0–9999 (reachable from DB-editable
  timestamps) now saturate at the representable bound, e.g. year 10000 renders
  as `9999-12-31T23:59:59Z` / `Fri, 31 Dec 9999 23:59:59 +0000` (issue #3093).
