-- Accounts. `tenant_id` is the organisation a user belongs to; it is the value
-- every tenant-scoped query is filtered by. Users are looked up by email at
-- login (across tenants — you don't know the tenant until after authentication),
-- so this table is intentionally NOT tenant-scoped.
CREATE TABLE users (
    id            BIGSERIAL PRIMARY KEY,
    email         TEXT      NOT NULL UNIQUE,
    password_hash TEXT      NOT NULL,
    tenant_id     TEXT      NOT NULL,
    created_at    TIMESTAMP NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_users_tenant ON users (tenant_id);

-- The tenant-scoped domain table. Every row carries `tenant_id`; the
-- `#[repository(Project, tenant_scoped)]` macro fills it in on insert and
-- filters every read by the current tenant, so one organisation can never see
-- another's projects.
CREATE TABLE projects (
    id         BIGSERIAL PRIMARY KEY,
    tenant_id  TEXT      NOT NULL,
    name       TEXT      NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_projects_tenant ON projects (tenant_id);
