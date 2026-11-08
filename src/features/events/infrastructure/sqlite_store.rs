//! `EventStore` on SQLite (`events.db`, `0600`, WAL, parameterized queries only).

use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use std::cell::RefCell;
use std::path::Path;

use crate::features::events::domain::{
    Event, EventKind, EventStatus, EventStore, NewEvent, PaneContext, Runtime, StoreError,
};
use crate::shared::infrastructure::fs_secure::{touch_private, FsError};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
INSERT OR IGNORE INTO meta (key, value) VALUES ('next_seq', 1);
CREATE TABLE IF NOT EXISTS events (
    id          TEXT PRIMARY KEY,
    seq         INTEGER NOT NULL UNIQUE,
    source      TEXT NOT NULL,
    agent       TEXT NOT NULL,
    kind        TEXT NOT NULL,
    status      TEXT NOT NULL,
    title       TEXT NOT NULL,
    summary     TEXT NOT NULL,
    pane        TEXT,
    runtime     TEXT,
    created_at  INTEGER NOT NULL,
    resolved_at INTEGER,
    resolution  TEXT
);
CREATE INDEX IF NOT EXISTS events_pane ON events (pane, kind, created_at);
CREATE INDEX IF NOT EXISTS events_created ON events (created_at);
";

/// Columns added after v0.2.0 (nullable, so old rows stay valid).
const ADDED_COLUMNS: &[(&str, &str)] = &[
    ("pane_session", "TEXT"),
    ("pane_pid", "INTEGER"),
    ("agent_seq", "INTEGER"),
];

const COLUMNS: &str =
    "id, seq, source, agent, kind, status, title, summary, pane, runtime, created_at, resolved_at, resolution, pane_session, pane_pid, agent_seq";

/// Adds the columns an older database lacks. Idempotent.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let existing: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('events')")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for (name, ty) in ADDED_COLUMNS {
        if !existing.iter().any(|c| c == name) {
            // Names and types are constants, never input.
            conn.execute_batch(&format!("ALTER TABLE events ADD COLUMN {name} {ty}"))?;
        }
    }
    Ok(())
}

pub struct SqliteEventStore {
    conn: RefCell<Connection>,
}

fn db_err(_: rusqlite::Error) -> StoreError {
    StoreError::Storage
}

fn row_to_event(r: &Row<'_>) -> rusqlite::Result<Event> {
    let kind: String = r.get(4)?;
    let status: String = r.get(5)?;
    let runtime: Option<String> = r.get(9)?;
    Ok(Event {
        id: r.get(0)?,
        seq: r.get::<_, i64>(1)? as u64,
        source: r.get(2)?,
        agent: r.get(3)?,
        kind: EventKind::parse(&kind).unwrap_or(EventKind::Question),
        status: if status == "resolved" {
            EventStatus::Resolved
        } else {
            EventStatus::Pending
        },
        title: r.get(6)?,
        summary: r.get(7)?,
        pane: r.get(8)?,
        runtime: runtime.as_deref().and_then(Runtime::parse),
        created_at: r.get::<_, i64>(10)? as u64,
        resolved_at: r.get::<_, Option<i64>>(11)?.map(|v| v as u64),
        resolution: r.get(12)?,
        context: PaneContext {
            session: r.get(13)?,
            pane_pid: r
                .get::<_, Option<i64>>(14)?
                .and_then(|v| u32::try_from(v).ok()),
            agent_seq: r
                .get::<_, Option<i64>>(15)?
                .and_then(|v| u64::try_from(v).ok()),
        },
    })
}

fn next_seq(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<i64> {
    let seq: i64 = tx.query_row("SELECT value FROM meta WHERE key = 'next_seq'", [], |r| {
        r.get(0)
    })?;
    tx.execute(
        "UPDATE meta SET value = value + 1 WHERE key = 'next_seq'",
        [],
    )?;
    Ok(seq)
}

impl SqliteEventStore {
    /// Opens (creating `0600` if needed) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, FsError> {
        touch_private(path)?;
        let io = |e: rusqlite::Error| {
            FsError::Io(
                path.display().to_string(),
                std::io::Error::other(e.to_string()),
            )
        };
        let conn = Connection::open(path).map_err(io)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(io)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(io)?;
        conn.pragma_update(None, "secure_delete", "ON")
            .map_err(io)?;
        conn.execute_batch(SCHEMA).map_err(io)?;
        migrate(&conn).map_err(io)?;
        Ok(Self {
            conn: RefCell::new(conn),
        })
    }

    fn query(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<Event>, StoreError> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(sql).map_err(db_err)?;
        let rows = stmt.query_map(p, row_to_event).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }
}

impl EventStore for SqliteEventStore {
    fn insert(&self, id: &str, e: &NewEvent) -> Result<Event, StoreError> {
        {
            let mut conn = self.conn.borrow_mut();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_err)?;
            let seq = next_seq(&tx).map_err(db_err)?;
            tx.execute(
                "INSERT INTO events (id, seq, source, agent, kind, status, title, summary, pane, runtime, created_at, pane_session, pane_pid, agent_seq)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    id,
                    seq,
                    e.source,
                    e.agent,
                    e.kind.as_str(),
                    e.title,
                    e.summary,
                    e.pane.as_ref().map(|p| p.pane.as_str()),
                    e.pane.as_ref().map(|p| p.runtime.as_str()),
                    e.created_at as i64,
                    e.pane.as_ref().and_then(|p| p.context.session.as_deref()),
                    e.pane.as_ref().and_then(|p| p.context.pane_pid),
                    e.pane
                        .as_ref()
                        .and_then(|p| p.context.agent_seq)
                        .and_then(|v| i64::try_from(v).ok()),
                ],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
        }
        self.get(id)?.ok_or(StoreError::Storage)
    }

    fn get(&self, id: &str) -> Result<Option<Event>, StoreError> {
        let conn = self.conn.borrow();
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM events WHERE id = ?1"),
            [id],
            row_to_event,
        )
        .optional()
        .map_err(db_err)
    }

    fn list_since(&self, since: u64, limit: u32) -> Result<Vec<Event>, StoreError> {
        self.query(
            &format!("SELECT {COLUMNS} FROM events WHERE seq > ?1 ORDER BY seq ASC LIMIT ?2"),
            params![since as i64, limit],
        )
    }

    fn last_for(&self, pane: &str, kind: EventKind) -> Result<Option<Event>, StoreError> {
        Ok(self
            .query(
                &format!(
                    "SELECT {COLUMNS} FROM events WHERE pane = ?1 AND kind = ?2 ORDER BY created_at DESC, seq DESC LIMIT 1"
                ),
                params![pane, kind.as_str()],
            )?
            .pop())
    }

    fn pending_for_pane(&self, pane: &str) -> Result<Vec<Event>, StoreError> {
        self.query(
            &format!("SELECT {COLUMNS} FROM events WHERE pane = ?1 AND status = 'pending' ORDER BY seq ASC"),
            [pane],
        )
    }

    fn resolve(&self, id: &str, resolution: &str, at: u64) -> Result<Event, StoreError> {
        {
            let mut conn = self.conn.borrow_mut();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db_err)?;
            let seq = next_seq(&tx).map_err(db_err)?;
            let n = tx
                .execute(
                    "UPDATE events SET status = 'resolved', seq = ?2, resolved_at = ?3, resolution = ?4
                     WHERE id = ?1 AND status = 'pending'",
                    params![id, seq, at as i64, resolution],
                )
                .map_err(db_err)?;
            if n != 1 {
                return Err(StoreError::NotFound);
            }
            tx.commit().map_err(db_err)?;
        }
        self.get(id)?.ok_or(StoreError::NotFound)
    }

    fn reopen(&self, id: &str) -> Result<(), StoreError> {
        let mut conn = self.conn.borrow_mut();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let seq = next_seq(&tx).map_err(db_err)?;
        let n = tx
            .execute(
                "UPDATE events SET status = 'pending', seq = ?2, resolved_at = NULL, resolution = NULL
                 WHERE id = ?1 AND status = 'resolved'",
                params![id, seq],
            )
            .map_err(db_err)?;
        if n != 1 {
            return Err(StoreError::NotFound);
        }
        tx.commit().map_err(db_err)
    }

    fn purge_before(&self, ts: u64) -> Result<u64, StoreError> {
        let conn = self.conn.borrow();
        conn.execute("DELETE FROM events WHERE created_at < ?1", [ts as i64])
            .map(|n| n as u64)
            .map_err(db_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::events::domain::PaneRef;
    use std::os::unix::fs::PermissionsExt;

    fn new(kind: EventKind, pane: &str, at: u64) -> NewEvent {
        NewEvent {
            source: "herdr".into(),
            agent: "claude".into(),
            kind,
            title: "t".into(),
            summary: "it's \"quoted\"; DROP TABLE events;--".into(),
            pane: Some(PaneRef::new(Runtime::Herdr, pane)),
            created_at: at,
        }
    }

    #[test]
    fn resolve_is_compare_and_set_across_connections() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("events.db");
        let a = SqliteEventStore::open(&p).unwrap();
        let b = SqliteEventStore::open(&p).unwrap();
        a.insert("e", &new(EventKind::ApprovalRequired, "w1:p1", 10))
            .unwrap();
        let won = a.resolve("e", "allow", 20).unwrap();
        assert!(matches!(
            b.resolve("e", "deny", 21),
            Err(StoreError::NotFound)
        ));
        let e = b.get("e").unwrap().unwrap();
        assert_eq!(e.resolution.as_deref(), Some("allow"));

        b.reopen("e").unwrap();
        let e = a.get("e").unwrap().unwrap();
        assert_eq!(e.status, EventStatus::Pending);
        assert_eq!((e.resolution, e.resolved_at), (None, None));
        assert!(e.seq > won.seq, "reopen gets a new seq");
        assert!(matches!(a.reopen("e"), Err(StoreError::NotFound)));
    }

    #[test]
    fn file_is_private_and_seq_is_monotonic_across_resolution() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("events.db");
        let s = SqliteEventStore::open(&p).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let a = s
            .insert("a", &new(EventKind::ApprovalRequired, "w1:p1", 10))
            .unwrap();
        let b = s.insert("b", &new(EventKind::Done, "w1:p2", 11)).unwrap();
        assert_eq!((a.seq, b.seq), (1, 2));
        assert_eq!(a.summary, "it's \"quoted\"; DROP TABLE events;--");
        let r = s.resolve("a", "allow", 12).unwrap();
        assert_eq!(r.seq, 3);
        assert_eq!(r.status, EventStatus::Resolved);
        assert_eq!(r.resolution.as_deref(), Some("allow"));
        assert!(matches!(
            s.resolve("a", "allow", 13),
            Err(StoreError::NotFound)
        ));
        let since: Vec<_> = s
            .list_since(2, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(since, vec!["a"]);
        assert_eq!(s.list_since(0, 1).unwrap().len(), 1);
    }

    #[test]
    fn seq_never_goes_back_after_purge() {
        let t = tempfile::tempdir().unwrap();
        let s = SqliteEventStore::open(&t.path().join("e.db")).unwrap();
        s.insert("a", &new(EventKind::Done, "p", 10)).unwrap();
        assert_eq!(s.purge_before(100).unwrap(), 1);
        assert_eq!(
            s.insert("b", &new(EventKind::Done, "p", 200)).unwrap().seq,
            2
        );
    }

    #[test]
    fn lookups_by_pane() {
        let t = tempfile::tempdir().unwrap();
        let s = SqliteEventStore::open(&t.path().join("e.db")).unwrap();
        s.insert("a", &new(EventKind::ApprovalRequired, "p", 10))
            .unwrap();
        s.insert("b", &new(EventKind::ApprovalRequired, "p", 20))
            .unwrap();
        s.insert("c", &new(EventKind::Done, "q", 20)).unwrap();
        assert_eq!(
            s.last_for("p", EventKind::ApprovalRequired)
                .unwrap()
                .unwrap()
                .id,
            "b"
        );
        assert!(s.last_for("p", EventKind::Done).unwrap().is_none());
        assert_eq!(s.pending_for_pane("p").unwrap().len(), 2);
        assert!(s.get("zzz").unwrap().is_none());
    }

    #[test]
    fn context_round_trips_and_old_databases_are_migrated() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("e.db");
        {
            // A v0.2.0 database: no context columns, one pending row.
            touch_private(&p).unwrap();
            let c = Connection::open(&p).unwrap();
            c.execute_batch(SCHEMA).unwrap();
            c.execute(
                "INSERT INTO events (id, seq, source, agent, kind, status, title, summary, pane, runtime, created_at)
                 VALUES ('old', 1, 'herdr', 'claude', 'approval_required', 'pending', 't', 's', 'w1:p1', 'herdr', 5)",
                [],
            )
            .unwrap();
            c.execute("UPDATE meta SET value = 2 WHERE key = 'next_seq'", [])
                .unwrap();
        }
        let s = SqliteEventStore::open(&p).unwrap();
        let old = s.get("old").unwrap().unwrap();
        assert_eq!(old.context, PaneContext::default());
        assert_eq!(old.pane_ref().unwrap().pane, "w1:p1");
        let mut n = new(EventKind::ApprovalRequired, "%3", 10);
        n.pane = Some(PaneRef {
            runtime: Runtime::Tmux,
            pane: "%3".into(),
            context: PaneContext {
                session: Some("/tmp/tmux-1000/default".into()),
                pane_pid: Some(4242),
                agent_seq: Some(21),
            },
        });
        let e = s.insert("new", &n).unwrap();
        assert_eq!(e.context.pane_pid, Some(4242));
        assert_eq!(e.pane_ref().unwrap(), n.pane.unwrap());
        // Reopening is idempotent.
        drop(s);
        assert!(SqliteEventStore::open(&p).is_ok());
    }

    #[test]
    fn loose_existing_db_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("e.db");
        std::fs::write(&p, b"").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(SqliteEventStore::open(&p).is_err());
    }
}
