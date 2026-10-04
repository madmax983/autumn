### Fixed

- Custom domains: an offboarded domain's orders now keep counting against the
  deployment-wide hourly issuance budget across a restart. Removing a domain
  used to delete the only durable copy of its recent attempts, so an
  issue/offboard cycle around a deploy or a crash handed the shared ACME
  account quota back. Recent attempts now move to a deployment-wide ledger in
  the domain store when the record is deleted, and the boot-time budget
  hydration reads the ledger alongside the surviving records. Ledger entries
  age out with the hourly window, and a re-registered hostname is still not
  charged for its predecessor's per-domain budget.
