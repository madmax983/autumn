### Fixed

- **`deploy status --strict` now flags an unreadable installed kamal-proxy
  port as state drift (#2278):** a reachable redeployed host whose installed
  unit exists but whose `--http-port` could not be read used to render
  `proxy ?` and nothing else — while its NEXT deploy was guaranteed to
  refuse closed, because the redeploy path's concurrent public-port-change
  guard refuses an unprovable installed port. `fleet_drift` now pushes a
  state-drift reason for exactly that shape, so the row turns ⚠️ and
  `--strict` exits non-zero through the existing `drifted()` path.
  Deliberately silent for `InstalledProxyPort::Absent` (a never-deployed
  host has no unit to read) and for `Unreadable` on a `First`-mode host
  (no unit could have been installed yet).
