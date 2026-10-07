//! Operational registry. This module is the only place that runs SQL.
//!
//! Human files stay outside it. After a directory is imported, project
//! lifecycle, thread records, and the coordinator binding are rows here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::Digest;

use crate::ids::{self, ENV_LOCAL};

pub const SCHEMA_VERSION: i64 = 2;
pub const REGISTRY_FILE: &str = "registry.sqlite";

const SCHEMA: &str = r#"
CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

CREATE TABLE environments (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('local', 'ssh')),
    herdr_machine_id TEXT,
    label TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL CHECK (state IN ('resolved', 'unresolved'))
);
CREATE UNIQUE INDEX environments_one_local ON environments(kind) WHERE kind = 'local';
CREATE UNIQUE INDEX environments_machine ON environments(herdr_machine_id) WHERE herdr_machine_id IS NOT NULL;

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL,
    pending_slug TEXT,
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('active', 'paused', 'archived', 'deleted')),
    availability TEXT NOT NULL CHECK (availability IN ('available', 'missing', 'invalid')),
    primary_session_id TEXT,
    directory TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX projects_live_slug ON projects(slug) WHERE lifecycle != 'deleted';
CREATE UNIQUE INDEX projects_pending_slug ON projects(pending_slug)
    WHERE pending_slug IS NOT NULL AND lifecycle != 'deleted';
CREATE TRIGGER projects_pending_slug_free
BEFORE UPDATE OF pending_slug ON projects
FOR EACH ROW
WHEN NEW.pending_slug IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'slug is reserved')
    WHERE EXISTS (
        SELECT 1 FROM projects
        WHERE id != NEW.id
          AND lifecycle != 'deleted'
          AND (slug = NEW.pending_slug OR pending_slug = NEW.pending_slug)
    );
END;
CREATE TRIGGER projects_slug_not_reserved
BEFORE INSERT ON projects
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'slug is reserved')
    WHERE EXISTS (
        SELECT 1 FROM projects
        WHERE lifecycle != 'deleted' AND pending_slug = NEW.slug
    );
END;
CREATE TRIGGER projects_slug_update_not_reserved
BEFORE UPDATE OF slug ON projects
FOR EACH ROW
WHEN NEW.slug != OLD.slug
BEGIN
    SELECT RAISE(ABORT, 'slug is reserved')
    WHERE EXISTS (
        SELECT 1 FROM projects
        WHERE id != NEW.id
          AND lifecycle != 'deleted'
          AND (slug = NEW.slug OR pending_slug = NEW.slug)
    );
END;

CREATE TABLE project_slugs (
    project_id TEXT NOT NULL,
    slug TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (project_id, slug),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT
);

CREATE TABLE project_projections (
    project_id TEXT PRIMARY KEY,
    repositories_source_hash TEXT NOT NULL,
    last_reconciled_at TEXT NOT NULL,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT
);

CREATE TABLE repositories (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    path TEXT NOT NULL,
    machine_label TEXT NOT NULL DEFAULT '',
    environment_id TEXT NOT NULL,
    removed INTEGER NOT NULL DEFAULT 0,
    UNIQUE (id, project_id),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT,
    FOREIGN KEY (environment_id) REFERENCES environments(id) ON DELETE RESTRICT
);

CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    repository_id TEXT,
    environment_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    cwd TEXT NOT NULL DEFAULT '',
    worktree_root TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL DEFAULT '',
    base_ref TEXT NOT NULL DEFAULT '',
    ownership TEXT NOT NULL CHECK (ownership IN ('legacy', 'managed')),
    availability TEXT NOT NULL CHECK (availability IN ('available', 'missing', 'invalid')),
    archived_at TEXT,
    UNIQUE (id, project_id),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT,
    FOREIGN KEY (environment_id) REFERENCES environments(id) ON DELETE RESTRICT,
    FOREIGN KEY (repository_id, project_id) REFERENCES repositories(id, project_id) ON DELETE RESTRICT
);
CREATE UNIQUE INDEX workspaces_managed_root ON workspaces(environment_id, worktree_root)
    WHERE ownership = 'managed' AND worktree_root != '' AND archived_at IS NULL;
CREATE UNIQUE INDEX workspaces_one_place ON workspaces(project_id, environment_id, cwd, worktree_root)
    WHERE archived_at IS NULL;

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    environment_id TEXT NOT NULL,
    herdr_socket TEXT NOT NULL,
    pane_id TEXT NOT NULL DEFAULT '',
    terminal_id TEXT NOT NULL DEFAULT '',
    workspace_herdr_id TEXT NOT NULL DEFAULT '',
    tab_id TEXT NOT NULL DEFAULT '',
    agent_name TEXT NOT NULL DEFAULT '',
    agent_kind TEXT NOT NULL DEFAULT '',
    profile TEXT NOT NULL DEFAULT '',
    agent_session TEXT NOT NULL DEFAULT '',
    cwd TEXT NOT NULL DEFAULT '',
    herdr_session_name TEXT NOT NULL DEFAULT '',
    role TEXT NOT NULL CHECK (role IN ('primary', 'secondary')),
    stale INTEGER NOT NULL DEFAULT 0,
    unbound_at TEXT,
    updated_at TEXT NOT NULL,
    UNIQUE (id, project_id),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT,
    FOREIGN KEY (environment_id) REFERENCES environments(id) ON DELETE RESTRICT
);
CREATE UNIQUE INDEX sessions_live_pane ON sessions(environment_id, herdr_socket, pane_id)
    WHERE pane_id != '' AND unbound_at IS NULL;

CREATE TRIGGER projects_primary_on_update
BEFORE UPDATE OF primary_session_id ON projects
FOR EACH ROW
WHEN NEW.primary_session_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'primary session is not in this project')
    WHERE NOT EXISTS (
        SELECT 1 FROM sessions WHERE id = NEW.primary_session_id AND project_id = NEW.id
    );
END;

CREATE TRIGGER projects_primary_on_insert
BEFORE INSERT ON projects
FOR EACH ROW
WHEN NEW.primary_session_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'primary session is not in this project')
    WHERE NOT EXISTS (
        SELECT 1 FROM sessions WHERE id = NEW.primary_session_id AND project_id = NEW.id
    );
END;

CREATE TABLE threads (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    workspace_id TEXT,
    label TEXT NOT NULL,
    lifecycle TEXT NOT NULL,
    body TEXT NOT NULL,
    UNIQUE (id, project_id),
    UNIQUE (project_id, label),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id) REFERENCES workspaces(id, project_id) ON DELETE RESTRICT
);

CREATE TABLE operations (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    step TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'done', 'failed')),
    idempotency_key TEXT NOT NULL UNIQUE,
    error TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT
);
CREATE TRIGGER operations_payload_frozen
BEFORE UPDATE OF payload ON operations
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'operation payload is frozen')
    WHERE NEW.payload != OLD.payload;
END;

CREATE TABLE events (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    actor_kind TEXT NOT NULL,
    actor_id TEXT,
    action TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT,
    result TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '',
    at TEXT NOT NULL,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE RESTRICT,
    FOREIGN KEY (target_id, project_id) REFERENCES threads(id, project_id) ON DELETE RESTRICT
);

CREATE TABLE legacy_imports (
    path TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('imported', 'failed', 'ignored')),
    error TEXT NOT NULL DEFAULT '',
    project_id TEXT,
    last_attempt TEXT NOT NULL
);
"#;

const UPGRADE_V2: &str = r#"
ALTER TABLE projects ADD COLUMN pending_slug TEXT;
CREATE UNIQUE INDEX projects_pending_slug ON projects(pending_slug)
    WHERE pending_slug IS NOT NULL AND lifecycle != 'deleted';
CREATE TRIGGER projects_pending_slug_free
BEFORE UPDATE OF pending_slug ON projects
FOR EACH ROW
WHEN NEW.pending_slug IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'slug is reserved')
    WHERE EXISTS (
        SELECT 1 FROM projects
        WHERE id != NEW.id
          AND lifecycle != 'deleted'
          AND (slug = NEW.pending_slug OR pending_slug = NEW.pending_slug)
    );
END;
CREATE TRIGGER projects_slug_not_reserved
BEFORE INSERT ON projects
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'slug is reserved')
    WHERE EXISTS (
        SELECT 1 FROM projects
        WHERE lifecycle != 'deleted' AND pending_slug = NEW.slug
    );
END;
CREATE TRIGGER projects_slug_update_not_reserved
BEFORE UPDATE OF slug ON projects
FOR EACH ROW
WHEN NEW.slug != OLD.slug
BEGIN
    SELECT RAISE(ABORT, 'slug is reserved')
    WHERE EXISTS (
        SELECT 1 FROM projects
        WHERE id != NEW.id
          AND lifecycle != 'deleted'
          AND (slug = NEW.slug OR pending_slug = NEW.slug)
    );
END;
"#;

pub struct Store {
    conn: Connection,
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRow {
    pub id: String,
    pub slug: String,
    pub pending_slug: Option<String>,
    pub lifecycle: String,
    pub availability: String,
    pub primary_session_id: Option<String>,
    pub directory: String,
}

pub struct OperationRow {
    pub id: String,
    pub project_id: String,
    pub kind: String,
    pub payload: String,
    pub step: String,
    pub status: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub project_id: String,
    pub environment_id: String,
    pub herdr_socket: String,
    pub pane_id: String,
    pub terminal_id: String,
    pub workspace_herdr_id: String,
    pub tab_id: String,
    pub agent_name: String,
    pub agent_kind: String,
    pub profile: String,
    pub agent_session: String,
    pub cwd: String,
    pub herdr_session_name: String,
    pub role: String,
    pub stale: bool,
    pub unbound_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionDraft {
    pub environment_id: String,
    pub herdr_socket: String,
    pub pane_id: String,
    pub terminal_id: String,
    pub workspace_herdr_id: String,
    pub tab_id: String,
    pub agent_name: String,
    pub agent_kind: String,
    pub profile: String,
    pub agent_session: String,
    pub cwd: String,
    pub herdr_session_name: String,
}

#[derive(Debug, Clone)]
pub struct WorkspaceDraft {
    pub kind: String,
    pub cwd: String,
    pub worktree_root: String,
    pub branch: String,
    pub base_ref: String,
    pub ownership: String,
    pub environment_id: String,
    pub repository_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRow {
    pub id: String,
    pub project_id: String,
    pub label: String,
    pub lifecycle: String,
    pub workspace_id: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepoRow {
    pub id: String,
    pub path: String,
    pub machine_label: String,
    pub environment_id: String,
    pub removed: bool,
}

fn now() -> String {
    jiff::Timestamp::now()
        .round(jiff::Unit::Second)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    // journal_mode does not call the busy handler, so a second opener can
    // see "database is locked" while the first opener is still migrating.
    for _ in 0..50 {
        let mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        if mode.eq_ignore_ascii_case("wal") {
            return Ok(());
        }
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => return Ok(()),
            Err(error) if error.to_string().contains("locked") => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("could not set WAL within the busy timeout");
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("could not create {}", root.display()))?;
        let path = root.join(REGISTRY_FILE);
        let conn = Connection::open(&path)
            .with_context(|| format!("could not open {}", path.display()))?;
        configure(&conn)?;
        let store = Store {
            conn,
            root: root.to_path_buf(),
        };
        store.migrate()?;
        store.reconcile_observed()?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self) -> PathBuf {
        self.root.join(REGISTRY_FILE)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let created = (|| {
            let version: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
                [],
                |row| row.get(0),
            )?;
            if version == 0 {
                self.conn.execute_batch(SCHEMA)?;
                self.conn.execute(
                    "INSERT INTO environments (id, kind, herdr_machine_id, label, state) VALUES (?1, 'local', NULL, '', 'resolved')",
                    [ENV_LOCAL],
                )?;
                self.conn.execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                    params![SCHEMA_VERSION, now()],
                )?;
            } else {
                let applied: i64 = self.conn.query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                    [],
                    |row| row.get(0),
                )?;
                if applied == 1 {
                    self.conn.execute_batch(UPGRADE_V2)?;
                    self.conn.execute(
                        "INSERT INTO schema_migrations (version, applied_at) VALUES (2, ?1)",
                        [now()],
                    )?;
                }
            }
            Ok(())
        })();
        if let Err(error) = created {
            let _ = self.conn.execute_batch("ROLLBACK");
            return Err(error);
        }
        self.conn.execute_batch("COMMIT")?;
        let applied: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        if applied != SCHEMA_VERSION {
            bail!("registry schema is {applied}, this binary expects {SCHEMA_VERSION}");
        }
        configure(&self.conn)?;
        Ok(())
    }

    pub fn integrity(&self) -> Result<(String, Vec<String>)> {
        let check: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        let mut fk = Vec::new();
        let mut stmt = self.conn.prepare("PRAGMA foreign_key_check")?;
        let rows = stmt.query_map([], |row| {
            Ok(format!(
                "{} {} {} {}",
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?
            ))
        })?;
        for row in rows {
            fk.push(row?);
        }
        Ok((check, fk))
    }

    pub fn schema_sql(&self, name: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = ?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .map_err(Into::into)
    }

    pub fn insert_project(
        &self,
        slug: &str,
        directory: &str,
        lifecycle: &str,
    ) -> Result<ProjectRow> {
        let id = ids::ProjectId::new().to_string();
        let stamp = now();
        self.conn.execute(
            "INSERT INTO projects (id, slug, lifecycle, availability, primary_session_id, directory, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'available', NULL, ?4, ?5, ?5)",
            params![id, slug, lifecycle, directory, stamp],
        )?;
        self.project_by_id(&id)
    }

    pub fn project_by_slug(&self, slug: &str) -> Result<Option<ProjectRow>> {
        self.conn
            .query_row(
                "SELECT id, slug, pending_slug, lifecycle, availability, primary_session_id, directory FROM projects WHERE lifecycle != 'deleted' AND (slug = ?1 OR pending_slug = ?1)",
                [slug],
                read_project,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn project_by_id(&self, id: &str) -> Result<ProjectRow> {
        self.conn
            .query_row(
                "SELECT id, slug, pending_slug, lifecycle, availability, primary_session_id, directory FROM projects WHERE id = ?1",
                [id],
                read_project,
            )
            .with_context(|| format!("no project {id}"))
    }

    pub fn set_directory(&self, id: &str, directory: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET directory = ?1, updated_at = ?2 WHERE id = ?3",
            params![directory, now(), id],
        )?;
        Ok(())
    }

    pub fn set_lifecycle(&self, id: &str, lifecycle: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE projects SET lifecycle = ?1, updated_at = ?2 WHERE id = ?3",
            params![lifecycle, now(), id],
        )?;
        if n != 1 {
            bail!("no project {id}");
        }
        Ok(())
    }

    pub fn set_availability(&self, id: &str, availability: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE projects SET availability = ?1, updated_at = ?2 WHERE id = ?3",
            params![availability, now(), id],
        )?;
        if n != 1 {
            bail!("no project {id}");
        }
        Ok(())
    }

    pub fn set_slug(&self, id: &str, slug: &str, directory: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET slug = ?1, directory = ?2, updated_at = ?3 WHERE id = ?4",
            params![slug, directory, now(), id],
        )?;
        Ok(())
    }

    pub fn former_slugs(&self, project_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT slug FROM project_slugs WHERE project_id = ?1 ORDER BY ordinal")?;
        let rows = stmt.query_map([project_id], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn add_former_slug(&self, project_id: &str, slug: &str) -> Result<()> {
        let next: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM project_slugs WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO project_slugs (project_id, slug, ordinal) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            params![project_id, slug, next],
        )?;
        Ok(())
    }

    pub fn set_primary(&self, project_id: &str, session_id: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET primary_session_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![session_id, now(), project_id],
        )?;
        Ok(())
    }

    /// Binds a pane. A different `terminal_id` on the same live triple unbinds
    /// the old session and inserts a new one. The same triple and terminal
    /// returns the existing row.
    pub fn bind_session(
        &self,
        project_id: &str,
        draft: &SessionDraft,
        make_primary: bool,
    ) -> Result<SessionRow> {
        if !draft.pane_id.is_empty() {
            if let Some(live) =
                self.live_session(&draft.environment_id, &draft.herdr_socket, &draft.pane_id)?
            {
                if live.project_id != project_id {
                    bail!(
                        "pane {} on {} is already bound to another project",
                        draft.pane_id,
                        draft.herdr_socket
                    );
                }
                if live.terminal_id == draft.terminal_id {
                    if make_primary {
                        self.promote(&live)?;
                    }
                    return Ok(live);
                }
                self.conn.execute(
                    "UPDATE sessions SET unbound_at = ?1, stale = 1, role = 'secondary', updated_at = ?1 WHERE id = ?2",
                    params![now(), live.id],
                )?;
                if live.role == "primary" {
                    self.set_primary(project_id, None)?;
                }
            }
        }
        let id = ids::SessionId::new().to_string();
        let role = if make_primary { "primary" } else { "secondary" };
        let stamp = now();
        self.conn.execute(
            "INSERT INTO sessions (
                id, project_id, environment_id, herdr_socket, pane_id, terminal_id,
                workspace_herdr_id, tab_id, agent_name, agent_kind, profile, agent_session,
                cwd, herdr_session_name, role, stale, unbound_at, updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,0,NULL,?16)",
            params![
                id,
                project_id,
                draft.environment_id,
                draft.herdr_socket,
                draft.pane_id,
                draft.terminal_id,
                draft.workspace_herdr_id,
                draft.tab_id,
                draft.agent_name,
                draft.agent_kind,
                draft.profile,
                draft.agent_session,
                draft.cwd,
                draft.herdr_session_name,
                role,
                stamp,
            ],
        )?;
        if make_primary {
            self.set_primary(project_id, Some(&id))?;
        }
        self.session_by_id(&id)
    }

    fn promote(&self, session: &SessionRow) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET role = 'secondary', updated_at = ?1 WHERE project_id = ?2 AND role = 'primary' AND id != ?3",
            params![now(), session.project_id, session.id],
        )?;
        self.conn.execute(
            "UPDATE sessions SET role = 'primary', stale = 0, unbound_at = NULL, updated_at = ?1 WHERE id = ?2",
            params![now(), session.id],
        )?;
        self.set_primary(&session.project_id, Some(&session.id))?;
        Ok(())
    }

    pub fn live_session(
        &self,
        environment_id: &str,
        socket: &str,
        pane_id: &str,
    ) -> Result<Option<SessionRow>> {
        self.conn
            .query_row(
                "SELECT id, project_id, environment_id, herdr_socket, pane_id, terminal_id, workspace_herdr_id, tab_id, agent_name, agent_kind, profile, agent_session, cwd, herdr_session_name, role, stale, unbound_at
                 FROM sessions WHERE environment_id = ?1 AND herdr_socket = ?2 AND pane_id = ?3 AND unbound_at IS NULL",
                params![environment_id, socket, pane_id],
                read_session,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn session_by_id(&self, id: &str) -> Result<SessionRow> {
        self.conn
            .query_row(
                "SELECT id, project_id, environment_id, herdr_socket, pane_id, terminal_id, workspace_herdr_id, tab_id, agent_name, agent_kind, profile, agent_session, cwd, herdr_session_name, role, stale, unbound_at
                 FROM sessions WHERE id = ?1",
                [id],
                read_session,
            )
            .with_context(|| format!("no session {id}"))
    }

    pub fn primary_session(&self, project_id: &str) -> Result<Option<SessionRow>> {
        let Some(id) = self.project_by_id(project_id)?.primary_session_id else {
            return Ok(None);
        };
        Ok(Some(self.session_by_id(&id)?))
    }

    /// Updates the primary session in place when the pane triple is unchanged.
    /// A new pane or terminal goes through [`Self::bind_session`].
    pub fn save_primary(&self, project_id: &str, draft: &SessionDraft) -> Result<SessionRow> {
        if let Some(primary) = self.primary_session(project_id)? {
            let same_pane = primary.environment_id == draft.environment_id
                && primary.herdr_socket == draft.herdr_socket
                && primary.pane_id == draft.pane_id
                && primary.terminal_id == draft.terminal_id;
            if same_pane || draft.pane_id.is_empty() && primary.pane_id.is_empty() {
                self.conn.execute(
                    "UPDATE sessions SET workspace_herdr_id = ?1, tab_id = ?2, agent_name = ?3, agent_kind = ?4, profile = ?5, agent_session = ?6, cwd = ?7, herdr_session_name = ?8, updated_at = ?9 WHERE id = ?10",
                    params![
                        draft.workspace_herdr_id,
                        draft.tab_id,
                        draft.agent_name,
                        draft.agent_kind,
                        draft.profile,
                        draft.agent_session,
                        draft.cwd,
                        draft.herdr_session_name,
                        now(),
                        primary.id,
                    ],
                )?;
                return self.session_by_id(&primary.id);
            }
            if primary.unbound_at.is_none() && !primary.pane_id.is_empty() {
                self.conn.execute(
                    "UPDATE sessions SET unbound_at = ?1, stale = 1, role = 'secondary', updated_at = ?1 WHERE id = ?2",
                    params![now(), primary.id],
                )?;
                self.set_primary(project_id, None)?;
            }
        }
        self.bind_session(project_id, draft, true)
    }

    pub fn mark_session_stale(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET stale = 1, updated_at = ?1 WHERE id = ?2",
            params![now(), session_id],
        )?;
        Ok(())
    }

    pub fn ensure_workspace(&self, project_id: &str, draft: &WorkspaceDraft) -> Result<String> {
        if let Some(id) = self.conn.query_row(
            "SELECT id FROM workspaces WHERE project_id = ?1 AND environment_id = ?2 AND cwd = ?3 AND worktree_root = ?4 AND archived_at IS NULL",
            params![project_id, draft.environment_id, draft.cwd, draft.worktree_root],
            |row| row.get(0),
        ).optional()? {
            return Ok(id);
        }
        let id = ids::WorkspaceId::new().to_string();
        self.conn.execute(
            "INSERT INTO workspaces (
                id, project_id, repository_id, environment_id, kind, cwd, worktree_root, branch, base_ref, ownership, availability, archived_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'available',NULL)",
            params![
                id,
                project_id,
                draft.repository_id,
                draft.environment_id,
                draft.kind,
                draft.cwd,
                draft.worktree_root,
                draft.branch,
                draft.base_ref,
                draft.ownership,
            ],
        )?;
        Ok(id)
    }

    pub fn save_thread(
        &self,
        project_id: &str,
        label: &str,
        lifecycle: &str,
        workspace_id: Option<&str>,
        body: &str,
    ) -> Result<ThreadRow> {
        if let Some(existing) = self.thread_by_label(project_id, label)? {
            self.conn.execute(
                "UPDATE threads SET lifecycle = ?1, workspace_id = ?2, body = ?3 WHERE id = ?4 AND project_id = ?5",
                params![lifecycle, workspace_id, body, existing.id, project_id],
            )?;
            return self
                .thread_by_label(project_id, label)?
                .context("thread disappeared");
        }
        let id = ids::ThreadId::new().to_string();
        self.conn.execute(
            "INSERT INTO threads (id, project_id, workspace_id, label, lifecycle, body) VALUES (?1,?2,?3,?4,?5,?6)",
            params![id, project_id, workspace_id, label, lifecycle, body],
        )?;
        self.thread_by_label(project_id, label)?
            .context("thread insert was not readable")
    }

    pub fn thread_by_label(&self, project_id: &str, label: &str) -> Result<Option<ThreadRow>> {
        self.conn
            .query_row(
                "SELECT id, project_id, label, lifecycle, workspace_id, body FROM threads WHERE project_id = ?1 AND label = ?2",
                params![project_id, label],
                read_thread,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_threads(&self, project_id: &str) -> Result<Vec<ThreadRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, label, lifecycle, workspace_id, body FROM threads WHERE project_id = ?1 ORDER BY label",
        )?;
        let rows = stmt.query_map([project_id], read_thread)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn repos_hash(repos: &[(&str, &str)]) -> String {
        let mut lines: Vec<String> = repos
            .iter()
            .map(|(path, machine)| format!("{path}\n{machine}"))
            .collect();
        lines.sort();
        let raw = lines.join("\n");
        let digest = sha2::Sha256::digest(raw.as_bytes());
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn projection_hash(&self, project_id: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT repositories_source_hash FROM project_projections WHERE project_id = ?1",
                [project_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn reconcile_repos(&self, project_id: &str, repos: &[(&str, &str)]) -> Result<bool> {
        let hash = Self::repos_hash(repos);
        if self.projection_hash(project_id)?.as_deref() == Some(hash.as_str()) {
            return Ok(false);
        }
        let existing = self.repos(project_id)?;
        for (path, machine) in repos {
            if existing
                .iter()
                .any(|row| row.path == *path && row.machine_label == *machine && !row.removed)
            {
                continue;
            }
            if let Some(row) = existing
                .iter()
                .find(|row| row.path == *path && row.machine_label == *machine)
            {
                self.conn.execute(
                    "UPDATE repositories SET removed = 0 WHERE id = ?1",
                    [&row.id],
                )?;
                continue;
            }
            let env = if machine.is_empty() {
                ENV_LOCAL.to_string()
            } else {
                self.environment_for_label(machine)?.id
            };
            let id = ids::RepositoryId::new().to_string();
            self.conn.execute(
                "INSERT INTO repositories (id, project_id, path, machine_label, environment_id, removed) VALUES (?1,?2,?3,?4,?5,0)",
                params![id, project_id, path, machine, env],
            )?;
        }
        for row in &existing {
            if row.removed {
                continue;
            }
            if !repos
                .iter()
                .any(|(path, machine)| *path == row.path && *machine == row.machine_label)
            {
                self.conn.execute(
                    "UPDATE repositories SET removed = 1 WHERE id = ?1",
                    [&row.id],
                )?;
            }
        }
        self.conn.execute(
            "INSERT INTO project_projections (project_id, repositories_source_hash, last_reconciled_at) VALUES (?1,?2,?3)
             ON CONFLICT(project_id) DO UPDATE SET repositories_source_hash = excluded.repositories_source_hash, last_reconciled_at = excluded.last_reconciled_at",
            params![project_id, hash, now()],
        )?;
        Ok(true)
    }

    pub fn repos(&self, project_id: &str) -> Result<Vec<RepoRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, machine_label, environment_id, removed FROM repositories WHERE project_id = ?1 ORDER BY path, machine_label",
        )?;
        let rows = stmt.query_map([project_id], |row| {
            Ok(RepoRow {
                id: row.get(0)?,
                path: row.get(1)?,
                machine_label: row.get(2)?,
                environment_id: row.get(3)?,
                removed: row.get::<_, i64>(4)? != 0,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn environment_for_label(&self, label: &str) -> Result<EnvironmentRow> {
        if let Some(row) = self.conn.query_row(
            "SELECT id, kind, herdr_machine_id, label, state FROM environments WHERE label = ?1 AND kind = 'ssh'",
            [label],
            read_env,
        ).optional()? {
            return Ok(row);
        }
        let id = ids::EnvironmentId::new().to_string();
        self.conn.execute(
            "INSERT INTO environments (id, kind, herdr_machine_id, label, state) VALUES (?1, 'ssh', NULL, ?2, 'unresolved')",
            params![id, label],
        )?;
        self.conn
            .query_row(
                "SELECT id, kind, herdr_machine_id, label, state FROM environments WHERE id = ?1",
                [&id],
                read_env,
            )
            .map_err(Into::into)
    }

    pub fn resolve_environment(
        &self,
        label: &str,
        herdr_machine_id: &str,
    ) -> Result<EnvironmentRow> {
        let row = self.environment_for_label(label)?;
        self.conn.execute(
            "UPDATE environments SET herdr_machine_id = ?1, state = 'resolved', label = ?2 WHERE id = ?3",
            params![herdr_machine_id, label, row.id],
        )?;
        self.conn
            .query_row(
                "SELECT id, kind, herdr_machine_id, label, state FROM environments WHERE id = ?1",
                [&row.id],
                read_env,
            )
            .map_err(Into::into)
    }

    pub fn unresolved_environments(&self) -> Result<Vec<EnvironmentRow>> {
        let mut stmt = self.conn.prepare("SELECT id, kind, herdr_machine_id, label, state FROM environments WHERE state = 'unresolved'")?;
        let rows = stmt.query_map([], read_env)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn begin_operation(
        &self,
        project_id: &str,
        kind: &str,
        payload: &str,
        key: &str,
    ) -> Result<(String, bool)> {
        if let Some((id, status, existing_payload)) = self
            .conn
            .query_row(
                "SELECT id, status, payload FROM operations WHERE idempotency_key = ?1",
                [key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            .optional()?
        {
            if existing_payload != payload {
                bail!("idempotency key {key} already belongs to a different operation");
            }
            if status == "failed" {
                bail!("operation {id} failed");
            }
            return Ok((id, false));
        }
        let id = ids::OperationId::new().to_string();
        let stamp = now();
        self.conn.execute(
            "INSERT INTO operations (id, project_id, kind, payload, step, status, idempotency_key, error, created_at, updated_at)
             VALUES (?1,?2,?3,?4,'reserved','pending',?5,'',?6,?6)",
            params![id, project_id, kind, payload, key, stamp],
        )?;
        Ok((id, true))
    }

    pub fn start_intent(&self, project_id: &str, kind: &str, payload: &str) -> Result<(String, bool)> {
        if let Some(existing) = self.pending_kind(project_id, kind)? {
            if existing.payload != payload {
                bail!(
                    "pending {kind} {} has a different intent",
                    existing.id
                );
            }
            return Ok((existing.id, false));
        }
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM operations WHERE project_id = ?1 AND kind = ?2",
            params![project_id, kind],
            |row| row.get(0),
        )?;
        let key = format!("{kind}:{project_id}:{}", n + 1);
        self.begin_operation(project_id, kind, payload, &key)
    }

    pub fn pending_kind(&self, project_id: &str, kind: &str) -> Result<Option<OperationRow>> {
        self.conn
            .query_row(
                "SELECT id, project_id, kind, payload, step, status, idempotency_key FROM operations WHERE project_id = ?1 AND kind = ?2 AND status = 'pending'",
                params![project_id, kind],
                read_operation,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_pending(&self) -> Result<Vec<OperationRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, kind, payload, step, status, idempotency_key FROM operations WHERE status = 'pending' ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], read_operation)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn reserve_slug(&self, id: &str, pending_slug: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE projects SET pending_slug = ?1, updated_at = ?2 WHERE id = ?3 AND lifecycle != 'deleted'",
            params![pending_slug, now(), id],
        )?;
        if n != 1 {
            bail!("no project {id}");
        }
        Ok(())
    }

    pub fn finish_rename(&self, id: &str, directory: &str) -> Result<()> {
        let row = self.project_by_id(id)?;
        let next = row.pending_slug.clone().unwrap_or(row.slug.clone());
        let n = self.conn.execute(
            "UPDATE projects SET slug = ?1, pending_slug = NULL, directory = ?2, updated_at = ?3 WHERE id = ?4",
            params![next, directory, now(), id],
        )?;
        if n != 1 {
            bail!("no project {id}");
        }
        Ok(())
    }

    /// Finish database rows when the filesystem effect is already present.
    /// Does not create, rename, or trash a directory.
    pub fn reconcile_observed(&self) -> Result<()> {
        for operation in self.list_pending()? {
            match operation.kind.as_str() {
                "create_project" => self.observe_create(&operation)?,
                "rename_project" => self.observe_rename(&operation)?,
                "delete_project" => self.observe_delete(&operation)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn observe_create(&self, operation: &OperationRow) -> Result<()> {
        let value: serde_json::Value = serde_json::from_str(&operation.payload)?;
        let Some(directory) = value.get("directory").and_then(|v| v.as_str()) else {
            return Ok(());
        };
        let text = std::fs::read_to_string(Path::new(directory).join("PROJECT.md")).ok();
        if text.as_deref().is_some_and(|text| crate::project::parse_project_md(text).is_ok()) {
            self.finish_operation(&operation.id, "done", "")?;
        }
        Ok(())
    }

    fn observe_rename(&self, operation: &OperationRow) -> Result<()> {
        let value: serde_json::Value = serde_json::from_str(&operation.payload)?;
        let (Some(from), Some(old_dir), Some(new_dir)) = (
            value.get("from").and_then(|v| v.as_str()),
            value.get("old_directory").and_then(|v| v.as_str()),
            value.get("new_directory").and_then(|v| v.as_str()),
        ) else {
            return Ok(());
        };
        let old_present = Path::new(old_dir).join("PROJECT.md").is_file();
        let new_present = Path::new(new_dir).join("PROJECT.md").is_file();
        if new_present && !old_present {
            self.finish_rename(&operation.project_id, new_dir)?;
            self.add_former_slug(&operation.project_id, from)?;
            self.finish_operation(&operation.id, "done", "")?;
        }
        Ok(())
    }

    fn observe_delete(&self, operation: &OperationRow) -> Result<()> {
        let value: serde_json::Value = serde_json::from_str(&operation.payload)?;
        let (Some(source), Some(trash)) = (
            value.get("source").and_then(|v| v.as_str()),
            value.get("trash").and_then(|v| v.as_str()),
        ) else {
            return Ok(());
        };
        let source_present = Path::new(source).join("PROJECT.md").is_file();
        let trash_present = Path::new(trash).join("PROJECT.md").is_file();
        if trash_present && !source_present {
            self.set_lifecycle(&operation.project_id, "deleted")?;
            self.conn.execute(
                "UPDATE projects SET directory = ?1, pending_slug = NULL, updated_at = ?2 WHERE id = ?3",
                params![trash, now(), operation.project_id],
            )?;
            self.finish_operation(&operation.id, "done", "")?;
        }
        Ok(())
    }

    pub fn finish_operation(&self, id: &str, status: &str, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE operations SET status = ?1, error = ?2, step = ?1, updated_at = ?3 WHERE id = ?4",
            params![status, error, now(), id],
        )?;
        Ok(())
    }

    pub fn record_event(
        &self,
        project_id: &str,
        actor_kind: &str,
        actor_id: Option<&str>,
        action: &str,
        target_kind: &str,
        target_id: Option<&str>,
        result: &str,
    ) -> Result<()> {
        // Events that name a thread use the composite foreign key. Other
        // targets leave target_id null so the key is not checked, and the
        // id is kept in detail-free columns via target_kind only when it is
        // not a thread. Callers pass a thread id only for target_kind thread.
        let thread_ref = if target_kind == "thread" {
            target_id
        } else {
            None
        };
        self.conn.execute(
            "INSERT INTO events (id, project_id, actor_kind, actor_id, action, target_kind, target_id, result, detail, at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'',?9)",
            params![ids::EventId::new().to_string(), project_id, actor_kind, actor_id, action, target_kind, thread_ref, result, now()],
        )?;
        Ok(())
    }

    pub fn note_import(
        &self,
        path: &str,
        status: &str,
        error: &str,
        project_id: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO legacy_imports (path, status, error, project_id, last_attempt) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(path) DO UPDATE SET status = excluded.status, error = excluded.error, project_id = excluded.project_id, last_attempt = excluded.last_attempt",
            params![path, status, error, project_id, now()],
        )?;
        Ok(())
    }

    pub fn import_status(&self, path: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT status FROM legacy_imports WHERE path = ?1",
                [path],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn failed_imports(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, error FROM legacy_imports WHERE status = 'failed' ORDER BY path",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn forget_failed_imports(&self) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM legacy_imports WHERE status = 'failed'", [])?)
    }

    /// One writer at a time. The second caller waits on the busy timeout, then
    /// sees the first caller's commit.
    pub fn immediate<T>(&self, body: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match body(self) {
            Ok(value) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentRow {
    pub id: String,
    pub kind: String,
    pub herdr_machine_id: Option<String>,
    pub label: String,
    pub state: String,
}

fn read_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperationRow> {
    Ok(OperationRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        kind: row.get(2)?,
        payload: row.get(3)?,
        step: row.get(4)?,
        status: row.get(5)?,
        idempotency_key: row.get(6)?,
    })
}

fn read_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRow> {
    Ok(ProjectRow {
        id: row.get(0)?,
        slug: row.get(1)?,
        pending_slug: row.get(2)?,
        lifecycle: row.get(3)?,
        availability: row.get(4)?,
        primary_session_id: row.get(5)?,
        directory: row.get(6)?,
    })
}

fn read_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        environment_id: row.get(2)?,
        herdr_socket: row.get(3)?,
        pane_id: row.get(4)?,
        terminal_id: row.get(5)?,
        workspace_herdr_id: row.get(6)?,
        tab_id: row.get(7)?,
        agent_name: row.get(8)?,
        agent_kind: row.get(9)?,
        profile: row.get(10)?,
        agent_session: row.get(11)?,
        cwd: row.get(12)?,
        herdr_session_name: row.get(13)?,
        role: row.get(14)?,
        stale: row.get::<_, i64>(15)? != 0,
        unbound_at: row.get(16)?,
    })
}

fn read_thread(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadRow> {
    Ok(ThreadRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        label: row.get(2)?,
        lifecycle: row.get(3)?,
        workspace_id: row.get(4)?,
        body: row.get(5)?,
    })
}

fn read_env(row: &rusqlite::Row<'_>) -> rusqlite::Result<EnvironmentRow> {
    Ok(EnvironmentRow {
        id: row.get(0)?,
        kind: row.get(1)?,
        herdr_machine_id: row.get(2)?,
        label: row.get(3)?,
        state: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (dir, store)
    }

    fn project(store: &Store, slug: &str) -> ProjectRow {
        store
            .insert_project(slug, &format!("/tmp/{slug}"), "active")
            .unwrap()
    }

    fn draft(socket: &str, pane: &str, terminal: &str) -> SessionDraft {
        SessionDraft {
            environment_id: ENV_LOCAL.into(),
            herdr_socket: socket.into(),
            pane_id: pane.into(),
            terminal_id: terminal.into(),
            workspace_herdr_id: "w1".into(),
            tab_id: "w1:t1".into(),
            agent_name: "hp".into(),
            agent_kind: "claude".into(),
            profile: "claude".into(),
            agent_session: String::new(),
            cwd: "/p".into(),
            herdr_session_name: String::new(),
        }
    }

    #[test]
    fn schema_has_the_ownership_constraints() {
        let (_dir, store) = fresh();
        let version = store.schema_version().unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let threads = store.schema_sql("threads").unwrap().unwrap();
        assert!(threads.contains(
            "FOREIGN KEY (workspace_id, project_id) REFERENCES workspaces(id, project_id)"
        ));
        assert!(threads.contains("ON DELETE RESTRICT"));
        assert!(threads.contains("UNIQUE (project_id, label)"));
        let workspaces = store.schema_sql("workspaces").unwrap().unwrap();
        assert!(workspaces.contains(
            "FOREIGN KEY (repository_id, project_id) REFERENCES repositories(id, project_id)"
        ));
        let managed = store
            .schema_sql("workspaces_managed_root")
            .unwrap()
            .unwrap();
        assert!(managed.contains("ownership = 'managed'"));
        let live = store.schema_sql("sessions_live_pane").unwrap().unwrap();
        assert!(
            live.contains("environment_id")
                && live.contains("herdr_socket")
                && live.contains("pane_id")
        );
        let (check, fk) = store.integrity().unwrap();
        assert_eq!(check, "ok");
        assert!(fk.is_empty());
    }

    #[test]
    fn cross_project_workspace_and_primary_are_rejected() {
        let (_dir, store) = fresh();
        let a = project(&store, "a");
        let b = project(&store, "b");
        let workspace = store
            .ensure_workspace(
                &b.id,
                &WorkspaceDraft {
                    kind: "directory".into(),
                    cwd: "/b".into(),
                    worktree_root: String::new(),
                    branch: String::new(),
                    base_ref: String::new(),
                    ownership: "legacy".into(),
                    environment_id: ENV_LOCAL.into(),
                    repository_id: None,
                },
            )
            .unwrap();
        let err = store
            .save_thread(&a.id, "t-0001", "running", Some(&workspace), "{}")
            .unwrap_err();
        assert!(
            err.to_string().contains("FOREIGN KEY") || format!("{err:?}").contains("constraint"),
            "{err:?}"
        );

        let session = store
            .bind_session(&b.id, &draft("/b.sock", "w1:p1", "t-old"), true)
            .unwrap();
        let err = store.set_primary(&a.id, Some(&session.id)).unwrap_err();
        assert!(
            format!("{err:?}").contains("primary session")
                || format!("{err:?}").contains("constraint"),
            "{err:?}"
        );
    }

    #[test]
    fn same_pane_on_two_sockets_is_allowed_and_terminal_reuse_rebounds() {
        let (_dir, store) = fresh();
        let a = project(&store, "a");
        let b = project(&store, "b");
        let first = store
            .bind_session(&a.id, &draft("/a.sock", "w1:p1", "old"), true)
            .unwrap();
        let other = store
            .bind_session(&b.id, &draft("/b.sock", "w1:p1", "old"), true)
            .unwrap();
        assert_ne!(first.id, other.id);
        let rebound = store
            .bind_session(&a.id, &draft("/a.sock", "w1:p1", "new"), true)
            .unwrap();
        assert_ne!(rebound.id, first.id);
        let old = store.session_by_id(&first.id).unwrap();
        assert!(old.unbound_at.is_some());
        assert!(old.stale);
        let primary = store.primary_session(&a.id).unwrap().unwrap();
        assert_eq!(primary.id, rebound.id);
    }

    #[test]
    fn two_projects_may_share_a_checkout_but_not_a_managed_root() {
        let (_dir, store) = fresh();
        let a = project(&store, "a");
        let b = project(&store, "b");
        let shared = WorkspaceDraft {
            kind: "primary_checkout".into(),
            cwd: "/repo".into(),
            worktree_root: String::new(),
            branch: "main".into(),
            base_ref: "origin/main".into(),
            ownership: "legacy".into(),
            environment_id: ENV_LOCAL.into(),
            repository_id: None,
        };
        assert!(store.ensure_workspace(&a.id, &shared).is_ok());
        assert!(store.ensure_workspace(&b.id, &shared).is_ok());
        let managed = WorkspaceDraft {
            kind: "worktree".into(),
            cwd: "/managed/one".into(),
            worktree_root: "/managed/one".into(),
            branch: "hp/a/t-0001".into(),
            base_ref: "origin/main".into(),
            ownership: "managed".into(),
            environment_id: ENV_LOCAL.into(),
            repository_id: None,
        };
        assert!(store.ensure_workspace(&a.id, &managed).is_ok());
        assert!(store.ensure_workspace(&b.id, &managed).is_err());
    }

    #[test]
    fn duplicate_slug_and_label_are_rejected_and_done_project_stays_active_when_missing() {
        let (_dir, store) = fresh();
        let a = project(&store, "a");
        assert!(store.insert_project("a", "/other", "active").is_err());
        store
            .save_thread(&a.id, "t-0001", "running", None, "{}")
            .unwrap();
        assert!(
            store
                .save_thread(&a.id, "t-0001", "running", None, "{}")
                .is_ok()
        );
        store.set_availability(&a.id, "missing").unwrap();
        let row = store.project_by_id(&a.id).unwrap();
        assert_eq!(row.lifecycle, "active");
        assert_eq!(row.availability, "missing");
    }

    #[test]
    fn operation_retry_keeps_one_row_and_payload_cannot_change() {
        let (_dir, store) = fresh();
        let a = project(&store, "a");
        let (id, created) = store
            .begin_operation(&a.id, "create_project", "{\"slug\":\"a\"}", "create:a")
            .unwrap();
        assert!(created);
        let (again, created) = store
            .begin_operation(&a.id, "create_project", "{\"slug\":\"a\"}", "create:a")
            .unwrap();
        assert!(!created);
        assert_eq!(id, again);
        let err = store
            .conn
            .execute(
                "UPDATE operations SET payload = 'other' WHERE id = ?1",
                [&id],
            )
            .unwrap_err();
        assert!(
            format!("{err:?}").contains("frozen") || format!("{err:?}").contains("ABORT"),
            "{err:?}"
        );
    }

    fn is_constraint(err: &anyhow::Error) -> bool {
        let text = format!("{err:?}");
        text.contains("constraint")
            || text.contains("FOREIGN")
            || text.contains("UNIQUE")
            || text.contains("ABORT")
    }

    #[test]
    fn sqlite_rejects_cross_project_and_duplicate_rows() {
        let (_dir, store) = fresh();
        let a = project(&store, "a");
        let b = project(&store, "b");
        store.reconcile_repos(&b.id, &[("/repo", "")]).unwrap();
        let repo_id = store.repos(&b.id).unwrap()[0].id.clone();
        let err = store
            .ensure_workspace(
                &a.id,
                &WorkspaceDraft {
                    kind: "directory".into(),
                    cwd: "/a-only".into(),
                    worktree_root: String::new(),
                    branch: String::new(),
                    base_ref: String::new(),
                    ownership: "legacy".into(),
                    environment_id: ENV_LOCAL.into(),
                    repository_id: Some(repo_id),
                },
            )
            .unwrap_err();
        assert!(is_constraint(&err), "{err:?}");

        store
            .save_thread(&a.id, "t-0001", "running", None, "{}")
            .unwrap();
        let err = store
            .conn
            .execute(
                "INSERT INTO threads (id, project_id, workspace_id, label, lifecycle, body) VALUES ('thr_dup', ?1, NULL, 't-0001', 'running', '{}')",
                [&a.id],
            )
            .unwrap_err();
        assert!(format!("{err:?}").contains("UNIQUE"), "{err:?}");

        let err = store
            .conn
            .execute(
                "INSERT INTO projects (id, slug, lifecycle, availability, directory, created_at, updated_at) VALUES ('prj_dup', 'a', 'active', 'available', '/dup', 't', 't')",
                [],
            )
            .unwrap_err();
        assert!(format!("{err:?}").contains("UNIQUE"), "{err:?}");

        store
            .bind_session(&a.id, &draft("/a.sock", "w1:p1", "term"), true)
            .unwrap();
        let err = store
            .conn
            .execute(
                "INSERT INTO sessions (id, project_id, environment_id, herdr_socket, pane_id, terminal_id, role, updated_at) VALUES ('ses_dup', ?1, 'env_local', '/a.sock', 'w1:p1', 'other', 'secondary', 't')",
                [&a.id],
            )
            .unwrap_err();
        assert!(format!("{err:?}").contains("UNIQUE"), "{err:?}");

        store
            .ensure_workspace(
                &a.id,
                &WorkspaceDraft {
                    kind: "worktree".into(),
                    cwd: "/managed/root".into(),
                    worktree_root: "/managed/root".into(),
                    branch: "hp/a/t-0002".into(),
                    base_ref: "main".into(),
                    ownership: "managed".into(),
                    environment_id: ENV_LOCAL.into(),
                    repository_id: None,
                },
            )
            .unwrap();
        let err = store
            .conn
            .execute(
                "INSERT INTO workspaces (id, project_id, environment_id, kind, cwd, worktree_root, ownership, availability) VALUES ('wks_dup', ?1, 'env_local', 'worktree', '/managed/root', '/managed/root', 'managed', 'available')",
                [&b.id],
            )
            .unwrap_err();
        assert!(format!("{err:?}").contains("UNIQUE"), "{err:?}");

        let (check, fk) = store.integrity().unwrap();
        assert_eq!(check, "ok");
        assert!(fk.is_empty(), "{fk:?}");
    }

    #[test]
    fn the_same_pane_id_on_two_environments_is_two_sessions() {
        let (_dir, store) = fresh();
        let a = project(&store, "a");
        let b = project(&store, "b");
        let remote = store.environment_for_label("box").unwrap();
        assert_eq!(remote.state, "unresolved");
        assert!(remote.herdr_machine_id.is_none());
        let local = store
            .bind_session(&a.id, &draft("/a.sock", "w1:p1", "term"), true)
            .unwrap();
        let mut other = draft("/a.sock", "w1:p1", "term");
        other.environment_id = remote.id;
        let remote_session = store.bind_session(&b.id, &other, true).unwrap();
        assert_ne!(local.id, remote_session.id);
        assert!(store.primary_session(&a.id).unwrap().unwrap().id == local.id);
        assert!(
            store.primary_session(&b.id).unwrap().unwrap().id == remote_session.id
        );
    }

    #[test]
    fn a_new_store_reads_what_the_closed_store_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let id = {
            let store = Store::open(dir.path()).unwrap();
            let row = project(&store, "a");
            store
                .begin_operation(&row.id, "create_project", "{\"slug\":\"a\"}", "create:a")
                .unwrap();
            row.id
        };
        let store = Store::open(dir.path()).unwrap();
        let row = store.project_by_slug("a").unwrap().unwrap();
        assert_eq!(row.id, id);
        let (again, created) = store
            .begin_operation(&row.id, "create_project", "{\"slug\":\"a\"}", "create:a")
            .unwrap();
        assert!(!created);
        let (check, fk) = store.integrity().unwrap();
        assert_eq!(check, "ok");
        assert!(fk.is_empty());
        let _ = again;
    }

    #[test]
    fn eight_connections_import_one_project_row() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("demo");
        std::fs::create_dir_all(folder.join(".state")).unwrap();
        std::fs::write(folder.join("PROJECT.md"), "+++\nname = \"Demo\"\n+++\n").unwrap();
        std::fs::write(
            folder.join(".state/project.json"),
            r#"{"status":"paused","former_slugs":["old"]}"#,
        )
        .unwrap();
        let mut handles = Vec::new();
        for _ in 0..8 {
            let root = dir.path().to_path_buf();
            handles.push(std::thread::spawn(move || {
                let project = crate::project::Project::load(&root, "demo").unwrap();
                project.open_row().map(|(_, row)| row.lifecycle)
            }));
        }
        let statuses: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().unwrap())
            .collect();
        assert!(
            statuses.iter().all(|status| status == "paused"),
            "{statuses:?}"
        );
        let store = Store::open(dir.path()).unwrap();
        let projects: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
            .unwrap();
        let imports: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM legacy_imports WHERE status = 'imported'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(projects, 1);
        assert_eq!(imports, 1);
        assert_eq!(store.former_slugs(&store.project_by_slug("demo").unwrap().unwrap().id).unwrap(), ["old"]);
        let (check, fk) = store.integrity().unwrap();
        assert_eq!(check, "ok");
        assert!(fk.is_empty(), "{fk:?}");
    }

    #[test]
    fn sql_and_legacy_files_stay_inside_their_modules() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sql = Vec::new();
        let mut legacy = Vec::new();
        let mut thread_toml = Vec::new();
        for entry in std::fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).unwrap();
            let text = text.split("\nmod tests").next().unwrap_or(&text);
            if text.contains("rusqlite") || text.contains("query_row") || text.contains("execute_batch")
            {
                sql.push(name.clone());
            }
            if text.contains("project.json") || text.contains("coordinator.json") {
                legacy.push(name.clone());
            }
            if text.contains("strip_suffix(\".toml\")") {
                thread_toml.push(name);
            }
        }
        sql.sort();
        legacy.sort();
        thread_toml.sort();
        assert_eq!(sql, ["store.rs"]);
        assert_eq!(legacy, ["project.rs"]);
        assert_eq!(thread_toml, ["thread.rs"]);
    }
}
