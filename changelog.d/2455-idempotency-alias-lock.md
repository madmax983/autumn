### Fixed

- Idempotency: the session layer now reserves the deferred commit's
  session-alias in-flight lock *before* persisting a mutated session. Between
  the session `save` (which makes a handler-changed tenancy key visible) and
  the deferred idempotency commit, a concurrent retry presenting the same
  unrotated session id previously found no lock on the alias key and re-ran
  the handler; it now observes a 409 in-flight conflict instead. A lock that
  is already held fails the request closed (503, primary lock retained)
  rather than clobbering another request's in-flight lock. (#2455)
