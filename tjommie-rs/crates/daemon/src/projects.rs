//! `projects.ts`: host repositories and project teams. Every method takes the `&Connection` the caller already holds
//! (like `getStoredBot(db, id)`), so a caller can run several steps in one lock section; never take the `Db` lock
//! inside these methods. Git runs through [`Worktrees`]; room storage comes in through [`RoomPort`] until `rooms.rs`
//! (M4) lands.

use std::collections::HashSet;
use std::sync::{Arc, MutexGuard, PoisonError};

use protocol::model::{Room, TEAM_ROOM_ID};
use protocol::projects::{
    Project, ProjectHeads, ProjectInput, ProjectPatch, ProjectRepository, ProjectState,
};
use protocol::{ErrorCode, Event};
use rusqlite::{Connection, OptionalExtension, Params, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::bots::{get_stored_bot, list_stored_bots};
use crate::context::Db;
use crate::ids::{new_id, now_iso};
use crate::server::RpcError;
use crate::worktrees::{Repository, Worktrees};

/// `Broadcast`: sends an event to every connected client.
pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// `assertAvailable`: refuses a bot whose lifecycle is changing (`daemon.ts:167`).
pub type AssertAvailable = Arc<dyn Fn(&str) -> Result<(), RpcError> + Send + Sync>;

/// The two room operations projects need (`rooms.ts` `getRoom` and `createRoom`). `rooms.rs` (M4) implements this;
/// until then [`SqlRooms`] runs the same SQL.
pub trait RoomPort: Send + Sync {
    fn get_room(&self, db: &Connection, id: &str) -> Result<Room, RpcError>;
    fn create_room(
        &self,
        db: &Connection,
        name: &str,
        bot_ids: &[String],
    ) -> Result<Room, RpcError>;
}

/// `getRoom` and `createRoom` (`rooms.ts:13-33`, with `assertNameFree` and `setMembers`).
pub struct SqlRooms;

impl RoomPort for SqlRooms {
    fn get_room(&self, db: &Connection, id: &str) -> Result<Room, RpcError> {
        let row: Option<(String, Option<String>)> = db
            .query_row(
                "SELECT name, created_at FROM conversations WHERE id = ?1 AND kind = 'room'",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (name, created_at) =
            row.ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such room"))?;
        let bot_ids = if id == TEAM_ROOM_ID {
            list_stored_bots(db)?
                .into_iter()
                .map(|bot| bot.id)
                .collect()
        } else {
            let mut statement = db.prepare(
                "SELECT b.id FROM bots b JOIN room_members m ON m.bot_id = b.id WHERE m.room_id = ?1 ORDER BY b.created_at, b.rowid",
            )?;
            statement
                .query_map([id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(Room {
            id: id.to_owned(),
            name,
            bot_ids,
            everyone: id == TEAM_ROOM_ID,
            created_at,
        })
    }

    fn create_room(
        &self,
        db: &Connection,
        name: &str,
        bot_ids: &[String],
    ) -> Result<Room, RpcError> {
        if exists(
            db,
            "SELECT id FROM conversations WHERE kind = 'room' AND lower(name) = lower(?1)",
            [name],
        )? {
            return Err(RpcError::new(
                ErrorCode::NameTaken,
                format!("There's already a room called {name}"),
            ));
        }
        let id = new_id();
        transaction(db, || {
            db.execute(
                "INSERT INTO conversations (id, kind, bot_id, name, created_at) VALUES (?1, 'room', NULL, ?2, ?3)",
                params![id, name, now_iso()],
            )?;
            let mut seen = HashSet::new();
            let unique: Vec<&String> = bot_ids.iter().filter(|bot| seen.insert(*bot)).collect();
            for bot in &unique {
                if !exists(db, "SELECT 1 FROM bots WHERE id = ?1", [bot.as_str()])? {
                    return Err(RpcError::new(
                        ErrorCode::BadParams,
                        "Room member: no such bot",
                    ));
                }
            }
            for bot in unique {
                db.execute(
                    "INSERT INTO room_members (room_id, bot_id) VALUES (?1, ?2)",
                    params![id, bot],
                )?;
            }
            Ok(())
        })?;
        self.get_room(db, &id)
    }
}

/// `transaction(db, work)`: `BEGIN`, then `COMMIT`, or `ROLLBACK` when `work` fails. Not reentrant, like the original.
pub fn transaction<T>(
    db: &Connection,
    work: impl FnOnce() -> Result<T, RpcError>,
) -> Result<T, RpcError> {
    db.execute_batch("BEGIN")?;
    match work() {
        Ok(value) => match db.execute_batch("COMMIT") {
            Ok(()) => Ok(value),
            Err(error) => {
                let _ = db.execute_batch("ROLLBACK");
                Err(error.into())
            }
        },
        Err(error) => {
            let _ = db.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// Whether the query finds a row.
pub fn exists(db: &Connection, sql: &str, params: impl Params) -> Result<bool, RpcError> {
    Ok(db.query_row(sql, params, |_| Ok(())).optional()?.is_some())
}

/// `storedJson` plus the schema parse: the stored `data` column as `T`, or `not_found`.
pub fn stored<T: DeserializeOwned>(row: Option<String>) -> Result<T, RpcError> {
    let data = row.ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such project record."))?;
    serde_json::from_str(&data)
        .map_err(|error| RpcError::new(ErrorCode::Internal, error.to_string()))
}

/// One `data` column, parsed (`SELECT data FROM ...`).
pub fn data_of<T: DeserializeOwned>(
    db: &Connection,
    sql: &str,
    params: impl Params,
) -> Result<T, RpcError> {
    stored(
        db.query_row(sql, params, |row| row.get::<_, String>(0))
            .optional()?,
    )
}

/// Every `data` column of the query, parsed in order.
pub fn data_list<T: DeserializeOwned>(
    db: &Connection,
    sql: &str,
    params: impl Params,
) -> Result<Vec<T>, RpcError> {
    let mut statement = db.prepare(sql)?;
    let rows = statement
        .query_map(params, |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(|row| stored(Some(row))).collect()
}

/// The text a lowercase serde enum is stored as (`"active"`, `"running"`).
pub fn wire_text<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `JSON.stringify(record)`.
pub fn to_json<T: Serialize>(value: &T) -> Result<String, RpcError> {
    serde_json::to_string(value)
        .map_err(|error| RpcError::new(ErrorCode::Internal, error.to_string()))
}

/// `text.slice(0, units)`: JavaScript counts UTF-16 code units.
pub fn slice_units(text: &str, units: usize) -> String {
    let kept: Vec<u16> = text.encode_utf16().take(units).collect();
    String::from_utf16_lossy(&kept)
}

pub struct Projects {
    pub db: Db,
    pub worktrees: Arc<Worktrees>,
    emit: Emit,
    assert_available: AssertAvailable,
    rooms: Arc<dyn RoomPort>,
}

impl Projects {
    /// `new Projects(db, worktrees, emit, assertAvailable)`: with a no-op `emit` and `assertAvailable`, which
    /// [`Projects::with_emit`] and [`Projects::with_assert_available`] replace.
    pub fn new(db: Db, worktrees: Arc<Worktrees>, rooms: Arc<dyn RoomPort>) -> Self {
        Self {
            db,
            worktrees,
            emit: Arc::new(|_| {}),
            assert_available: Arc::new(|_| Ok(())),
            rooms,
        }
    }

    pub fn with_emit(mut self, emit: Emit) -> Self {
        self.emit = emit;
        self
    }

    pub fn with_assert_available(mut self, assert_available: AssertAvailable) -> Self {
        self.assert_available = assert_available;
        self
    }

    /// The database, for a caller that runs several steps in one lock section.
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn emit(&self, event: Event) {
        (self.emit)(event);
    }

    pub fn rooms(&self) -> &Arc<dyn RoomPort> {
        &self.rooms
    }

    pub fn repositories(&self, db: &Connection) -> Result<Vec<ProjectRepository>, RpcError> {
        let mut statement =
            db.prepare("SELECT id, name, created_at FROM project_repositories ORDER BY rowid")?;
        let rows = statement
            .query_map([], |row| {
                Ok(ProjectRepository {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn repository(&self, db: &Connection, id: &str) -> Result<Repository, RpcError> {
        db.query_row(
            "SELECT id, path, common_dir, name, created_at FROM project_repositories WHERE id = ?1",
            [id],
            |row| {
                Ok(Repository {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    common_dir: row.get(2)?,
                    name: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such host repository."))
    }

    /// `register`: the repository's root folder, once per Git common directory.
    pub fn register(&self, db: &Connection, path: &str) -> Result<ProjectRepository, RpcError> {
        let info = self.worktrees.repository(path)?;
        let found: Option<String> = db
            .query_row(
                "SELECT id FROM project_repositories WHERE common_dir = ?1",
                [&info.common_dir],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = found {
            let saved = self.repository(db, &id)?;
            return Ok(ProjectRepository {
                id: saved.id,
                name: saved.name,
                created_at: saved.created_at,
            });
        }
        let (id, created_at) = (new_id(), now_iso());
        db.execute(
            "INSERT INTO project_repositories VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, info.path, info.common_dir, info.name, created_at],
        )?;
        Ok(ProjectRepository {
            id,
            name: info.name,
            created_at,
        })
    }

    pub fn list(&self, db: &Connection) -> Result<Vec<Project>, RpcError> {
        data_list(db, "SELECT data FROM projects ORDER BY rowid", [])
    }

    pub fn get(&self, db: &Connection, id: &str) -> Result<Project, RpcError> {
        data_of(db, "SELECT data FROM projects WHERE id = ?1", [id])
    }

    pub fn active(&self, db: &Connection, id: &str) -> Result<Project, RpcError> {
        let project = self.get(db, id)?;
        if project.state != ProjectState::Active {
            return Err(RpcError::new(
                ErrorCode::BadState,
                "This project is archived.",
            ));
        }
        Ok(project)
    }

    pub fn heads(&self, db: &Connection, id: &str) -> Result<ProjectHeads, RpcError> {
        let project = self.get(db, id)?;
        let repo = self.repository(db, &project.input.repository_id)?;
        Ok(ProjectHeads {
            base_sha: self.worktrees.resolve_ref(&repo, &project.input.base_ref)?,
            target_sha: self
                .worktrees
                .resolve_ref(&repo, &project.input.target_ref)?,
        })
    }

    /// A task attempt is active in the project, or one of its active members holds a lease.
    pub fn busy(&self, db: &Connection, project_id: &str) -> Result<bool, RpcError> {
        Ok(exists(
            db,
            "SELECT 1 FROM task_attempts a JOIN project_tasks t ON t.id = a.task_id WHERE t.project_id = ?1 AND a.state IN ('provisioning', 'queued', 'running') LIMIT 1",
            [project_id],
        )? || exists(
            db,
            "SELECT 1 FROM project_bot_leases l JOIN project_members m ON m.bot_id = l.bot_id WHERE m.project_id = ?1 AND m.active = 1 LIMIT 1",
            [project_id],
        )?)
    }

    pub fn bot_in_use(&self, db: &Connection, bot_id: &str) -> Result<bool, RpcError> {
        exists(
            db,
            "SELECT 1 FROM project_members WHERE bot_id = ?1 AND active = 1",
            [bot_id],
        )
    }

    pub fn assert_bot_idle(&self, db: &Connection, bot_id: &str) -> Result<(), RpcError> {
        (self.assert_available)(bot_id)?;
        if exists(
            db,
            "SELECT 1 FROM work_items WHERE bot_id = ?1 AND status IN ('queued', 'running') LIMIT 1",
            [bot_id],
        )? {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "This bot has queued or running work.",
            ));
        }
        if exists(
            db,
            "SELECT 1 FROM project_bot_leases WHERE bot_id = ?1",
            [bot_id],
        )? {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "This bot owns a task or check that has not finished.",
            ));
        }
        Ok(())
    }

    /// `#members`: each bot and role once, every bot idle and in no other active project, unique check ids.
    fn members(
        &self,
        db: &Connection,
        project: &Project,
        except: Option<&str>,
    ) -> Result<(), RpcError> {
        let mut ids = HashSet::new();
        for member in &project.input.members {
            let roles: HashSet<_> = member.roles.iter().collect();
            if !ids.insert(member.bot_id.as_str()) || roles.len() != member.roles.len() {
                return Err(RpcError::new(
                    ErrorCode::BadParams,
                    "Each bot and role must occur once.",
                ));
            }
            get_stored_bot(db, &member.bot_id)?;
            self.assert_bot_idle(db, &member.bot_id)?;
            if exists(
                db,
                "SELECT project_id FROM project_members WHERE bot_id = ?1 AND active = 1 AND project_id IS NOT ?2",
                params![member.bot_id, except],
            )? {
                return Err(RpcError::new(
                    ErrorCode::Busy,
                    "This bot already belongs to another active project.",
                ));
            }
        }
        let checks: HashSet<_> = project.input.checks.iter().map(|check| &check.id).collect();
        if checks.len() != project.input.checks.len() {
            return Err(RpcError::new(
                ErrorCode::BadParams,
                "Project check IDs must be unique.",
            ));
        }
        Ok(())
    }

    /// `#save`: the project row and its member bindings (`active` only while the project is).
    fn save(&self, db: &Connection, project: &Project) -> Result<(), RpcError> {
        db.execute(
            "UPDATE projects SET state = ?1, revision = ?2, data = ?3 WHERE id = ?4",
            params![
                wire_text(&project.state),
                project.revision as i64,
                to_json(project)?,
                project.id
            ],
        )?;
        db.execute(
            "UPDATE project_members SET active = 0 WHERE project_id = ?1",
            [&project.id],
        )?;
        let active = i64::from(project.state == ProjectState::Active);
        for member in &project.input.members {
            db.execute(
                "INSERT INTO project_members VALUES (?1, ?2, ?3) ON CONFLICT(project_id, bot_id) DO UPDATE SET active = excluded.active",
                params![project.id, member.bot_id, active],
            )?;
        }
        Ok(())
    }

    pub fn create(&self, db: &Connection, input: ProjectInput) -> Result<Project, RpcError> {
        let repo = self.repository(db, &input.repository_id)?;
        self.worktrees.resolve_ref(&repo, &input.base_ref)?;
        self.worktrees.resolve_ref(&repo, &input.target_ref)?;
        let timestamp = now_iso();
        let mut project = Project {
            input: input.clone(),
            id: new_id(),
            state: ProjectState::Active,
            revision: 1,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        self.members(db, &project, None)?;
        match project.input.room_id.clone() {
            Some(room_id) => {
                self.rooms.get_room(db, &room_id)?;
            }
            None => {
                let bot_ids: Vec<String> = project
                    .input
                    .members
                    .iter()
                    .map(|member| member.bot_id.clone())
                    .collect();
                let room =
                    self.rooms
                        .create_room(db, &slice_units(&project.input.name, 60), &bot_ids)?;
                project.input.room_id = Some(room.id);
            }
        }
        transaction(db, || {
            db.execute(
                "INSERT INTO projects VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    project.id,
                    project.input.repository_id,
                    wire_text(&project.state),
                    project.revision as i64,
                    to_json(&project)?
                ],
            )?;
            self.save(db, &project)
        })?;
        if let (None, Some(room_id)) = (&input.room_id, &project.input.room_id) {
            self.emit(Event::RoomUpdated(self.rooms.get_room(db, room_id)?));
        }
        self.emit(Event::ProjectUpdated(project.clone()));
        Ok(project)
    }

    pub fn update(
        &self,
        db: &Connection,
        id: &str,
        expected_revision: u64,
        patch: ProjectPatch,
    ) -> Result<Project, RpcError> {
        let previous = self.active(db, id)?;
        if previous.revision != expected_revision {
            return Err(RpcError::new(
                ErrorCode::BadState,
                "This project changed. Refresh it before saving.",
            ));
        }
        if self.busy(db, id)? {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "Stop project work before changing its configuration.",
            ));
        }
        let mut project = previous.clone();
        let input = &mut project.input;
        if let Some(name) = patch.name {
            input.name = name;
        }
        if let Some(base_ref) = patch.base_ref {
            input.base_ref = base_ref;
        }
        if let Some(target_ref) = patch.target_ref {
            input.target_ref = target_ref;
        }
        if let Some(room_id) = patch.room_id {
            input.room_id = room_id;
        }
        if let Some(members) = patch.members {
            input.members = members;
        }
        if let Some(checks) = patch.checks {
            input.checks = checks;
        }
        project.revision = previous.revision + 1;
        project.updated_at = now_iso();
        self.members(db, &project, Some(id))?;
        let repo = self.repository(db, &project.input.repository_id)?;
        self.worktrees.resolve_ref(&repo, &project.input.base_ref)?;
        self.worktrees
            .resolve_ref(&repo, &project.input.target_ref)?;
        if let Some(room_id) = &project.input.room_id {
            self.rooms.get_room(db, room_id)?;
        }
        transaction(db, || self.save(db, &project))?;
        self.emit(Event::ProjectUpdated(project.clone()));
        Ok(project)
    }

    pub fn archive(
        &self,
        db: &Connection,
        id: &str,
        expected_revision: u64,
    ) -> Result<Project, RpcError> {
        let previous = self.get(db, id)?;
        if previous.revision != expected_revision {
            return Err(RpcError::new(
                ErrorCode::BadState,
                "This project changed. Refresh it before archiving.",
            ));
        }
        if self.busy(db, id)? {
            return Err(RpcError::new(
                ErrorCode::Busy,
                "Stop project work before archiving its team.",
            ));
        }
        let project = Project {
            state: ProjectState::Archived,
            revision: previous.revision + 1,
            updated_at: now_iso(),
            ..previous
        };
        transaction(db, || self.save(db, &project))?;
        self.emit(Event::ProjectUpdated(project.clone()));
        Ok(project)
    }
}
