//! Application state kept in one SQLite file.
//!
//! What lives here is what a user would be annoyed to lose on restart: window
//! layout and toggles, the view/sort chosen for each folder, the tabs that were
//! open, the folders visited recently, and a cache of generated thumbnails.
//!
//! The store is deliberately dumb: strings in, strings out. Turning those into
//! typed settings is the caller's job, which keeps this crate free of any GUI or
//! file-manager types and easy to test against real temporary databases.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, params};

/// Errors carry what was being done and where, never a bare SQLite message.
#[derive(Debug)]
pub struct StoreError {
    pub op: &'static str,
    pub path: Option<PathBuf>,
    pub source: rusqlite::Error,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(path) => write!(f, "{} ({}): {}", self.op, path.display(), self.source),
            None => write!(f, "{}: {}", self.op, self.source),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

fn ctx(op: &'static str) -> impl FnOnce(rusqlite::Error) -> StoreError {
    move |source| StoreError {
        op,
        path: None,
        source,
    }
}

/// Schema migrations, applied in order. `PRAGMA user_version` records how many
/// have run, so a database from an older build upgrades in place.
const MIGRATIONS: &[&str] = &[
    // v1
    "
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE folder_prefs (
        path        TEXT PRIMARY KEY,
        view        TEXT,
        sort_key    TEXT,
        sort_desc   INTEGER NOT NULL DEFAULT 0,
        dirs_first  INTEGER NOT NULL DEFAULT 1,
        updated_at  INTEGER NOT NULL
    );
    CREATE TABLE session_tabs (
        pane     INTEGER NOT NULL,
        position INTEGER NOT NULL,
        path     TEXT NOT NULL,
        active   INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (pane, position)
    );
    CREATE TABLE recents (
        path       TEXT PRIMARY KEY,
        visits     INTEGER NOT NULL DEFAULT 1,
        last_visit INTEGER NOT NULL
    );
    CREATE INDEX recents_last_visit ON recents(last_visit DESC);
    CREATE TABLE thumbnails (
        key       TEXT PRIMARY KEY,
        png       BLOB NOT NULL,
        bytes     INTEGER NOT NULL,
        last_used INTEGER NOT NULL
    );
    CREATE INDEX thumbnails_last_used ON thumbnails(last_used);
    ",
];

/// View and sort remembered for one folder, as Finder does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPrefs {
    pub view: Option<String>,
    pub sort_key: Option<String>,
    pub sort_desc: bool,
    pub dirs_first: bool,
}

/// One tab of the saved session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabState {
    pub path: String,
    pub active: bool,
}

/// Rows kept in `recents`; older visits are pruned.
const MAX_RECENTS: usize = 200;

pub struct Store {
    conn: Mutex<Connection>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

impl Store {
    /// `~/Library/Application Support/Veloce/veloce.db` on macOS, the platform's
    /// data directory elsewhere. `None` if the platform has no such directory.
    pub fn default_path() -> Option<PathBuf> {
        dirs::data_dir().map(|dir| dir.join("Veloce").join("veloce.db"))
    }

    /// Open (creating and migrating) the database at `path`. A file that is not a
    /// database is moved aside to `<name>.corrupt` and replaced, so a damaged
    /// state file never stops the app from starting.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| StoreError {
                op: "create state directory",
                path: Some(parent.to_path_buf()),
                source: rusqlite::Error::ToSqlConversionFailure(Box::new(error)),
            })?;
        }
        match Self::open_exact(path) {
            Ok(store) => Ok(store),
            Err(error) if is_corruption(&error.source) => {
                let aside = path.with_extension("db.corrupt");
                let _ = std::fs::rename(path, &aside);
                // The write-ahead files belong to the broken database.
                for suffix in ["-wal", "-shm"] {
                    let mut name = path.as_os_str().to_os_string();
                    name.push(suffix);
                    let _ = std::fs::remove_file(PathBuf::from(name));
                }
                Self::open_exact(path)
            }
            Err(error) => Err(error),
        }
    }

    fn open_exact(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|source| StoreError {
            op: "open database",
            path: Some(path.to_path_buf()),
            source,
        })?;
        // WAL keeps readers and the single writer from blocking each other.
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
            .map_err(ctx("configure database"))?;
        Self::finish(conn)
    }

    /// A throwaway database, for tests and as the fallback when the real one
    /// cannot be opened (the app then simply does not remember anything).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(ctx("open in-memory database"))?;
        Self::finish(conn)
    }

    fn finish(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(2))
            .map_err(ctx("configure database"))?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A panic while holding the lock cannot leave SQLite half-written, so a
        // poisoned lock is still safe to use.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- settings ---------------------------------------------------------

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.lock()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(ctx("read setting"))
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.lock()
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
            .map_err(ctx("write setting"))
    }

    /// Several settings in one transaction, so a layout is saved all or nothing.
    pub fn set_settings(&self, entries: &[(&str, String)]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(ctx("write settings"))?;
        for (key, value) in entries {
            tx.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(ctx("write settings"))?;
        }
        tx.commit().map_err(ctx("write settings"))
    }

    // ---- per-folder preferences --------------------------------------------

    /// Every saved folder preference, loaded once at start-up so navigating never
    /// has to wait on the database.
    pub fn all_folder_prefs(&self) -> Result<Vec<(String, FolderPrefs)>> {
        let conn = self.lock();
        let mut statement = conn
            .prepare("SELECT path, view, sort_key, sort_desc, dirs_first FROM folder_prefs")
            .map_err(ctx("read folder preferences"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    FolderPrefs {
                        view: row.get(1)?,
                        sort_key: row.get(2)?,
                        sort_desc: row.get::<_, i64>(3)? != 0,
                        dirs_first: row.get::<_, i64>(4)? != 0,
                    },
                ))
            })
            .map_err(ctx("read folder preferences"))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(ctx("read folder preferences"))
    }

    pub fn set_folder_prefs(&self, path: &str, prefs: &FolderPrefs) -> Result<()> {
        self.lock()
            .execute(
                "INSERT INTO folder_prefs
                    (path, view, sort_key, sort_desc, dirs_first, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(path) DO UPDATE SET
                    view = excluded.view,
                    sort_key = excluded.sort_key,
                    sort_desc = excluded.sort_desc,
                    dirs_first = excluded.dirs_first,
                    updated_at = excluded.updated_at",
                params![
                    path,
                    prefs.view,
                    prefs.sort_key,
                    prefs.sort_desc as i64,
                    prefs.dirs_first as i64,
                    now()
                ],
            )
            .map(|_| ())
            .map_err(ctx("write folder preferences"))
    }

    // ---- session -----------------------------------------------------------

    /// Replace the saved tabs of one pane.
    pub fn save_tabs(&self, pane: u8, tabs: &[TabState]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(ctx("save session"))?;
        tx.execute("DELETE FROM session_tabs WHERE pane = ?1", [pane])
            .map_err(ctx("save session"))?;
        for (position, tab) in tabs.iter().enumerate() {
            tx.execute(
                "INSERT INTO session_tabs (pane, position, path, active)
                 VALUES (?1, ?2, ?3, ?4)",
                params![pane, position as i64, tab.path, tab.active as i64],
            )
            .map_err(ctx("save session"))?;
        }
        tx.commit().map_err(ctx("save session"))
    }

    pub fn load_tabs(&self, pane: u8) -> Result<Vec<TabState>> {
        let conn = self.lock();
        let mut statement = conn
            .prepare("SELECT path, active FROM session_tabs WHERE pane = ?1 ORDER BY position")
            .map_err(ctx("load session"))?;
        let rows = statement
            .query_map([pane], |row| {
                Ok(TabState {
                    path: row.get(0)?,
                    active: row.get::<_, i64>(1)? != 0,
                })
            })
            .map_err(ctx("load session"))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(ctx("load session"))
    }

    // ---- recents -----------------------------------------------------------

    pub fn record_visit(&self, path: &str) -> Result<()> {
        self.record_visit_at(path, now())
    }

    /// `record_visit` with an explicit clock, so ordering is testable.
    pub fn record_visit_at(&self, path: &str, at: i64) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO recents (path, visits, last_visit) VALUES (?1, 1, ?2)
             ON CONFLICT(path) DO UPDATE SET
                visits = visits + 1, last_visit = excluded.last_visit",
            params![path, at],
        )
        .map_err(ctx("record visit"))?;
        conn.execute(
            "DELETE FROM recents WHERE path NOT IN
                (SELECT path FROM recents ORDER BY last_visit DESC LIMIT ?1)",
            [MAX_RECENTS as i64],
        )
        .map(|_| ())
        .map_err(ctx("prune recents"))
    }

    /// Most recently visited folders first.
    pub fn recent_paths(&self, limit: usize) -> Result<Vec<String>> {
        let conn = self.lock();
        let mut statement = conn
            .prepare("SELECT path FROM recents ORDER BY last_visit DESC, rowid DESC LIMIT ?1")
            .map_err(ctx("read recents"))?;
        let rows = statement
            .query_map([limit as i64], |row| row.get(0))
            .map_err(ctx("read recents"))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(ctx("read recents"))
    }

    pub fn visit_count(&self, path: &str) -> Result<u32> {
        self.lock()
            .query_row(
                "SELECT visits FROM recents WHERE path = ?1",
                [path],
                |row| row.get(0),
            )
            .optional()
            .map(|count| count.unwrap_or(0))
            .map_err(ctx("read visit count"))
    }

    // ---- thumbnails --------------------------------------------------------

    pub fn put_thumbnail(&self, key: &str, png: &[u8]) -> Result<()> {
        self.put_thumbnail_at(key, png, now())
    }

    pub fn put_thumbnail_at(&self, key: &str, png: &[u8], at: i64) -> Result<()> {
        self.lock()
            .execute(
                "INSERT INTO thumbnails (key, png, bytes, last_used) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(key) DO UPDATE SET
                    png = excluded.png, bytes = excluded.bytes, last_used = excluded.last_used",
                params![key, png, png.len() as i64, at],
            )
            .map(|_| ())
            .map_err(ctx("write thumbnail"))
    }

    pub fn get_thumbnail(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.get_thumbnail_at(key, now())
    }

    /// Fetch a thumbnail and mark it used, which is what keeps it out of the
    /// least-recently-used pruning.
    pub fn get_thumbnail_at(&self, key: &str, at: i64) -> Result<Option<Vec<u8>>> {
        let conn = self.lock();
        let png: Option<Vec<u8>> = conn
            .query_row("SELECT png FROM thumbnails WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(ctx("read thumbnail"))?;
        if png.is_some() {
            conn.execute(
                "UPDATE thumbnails SET last_used = ?2 WHERE key = ?1",
                params![key, at],
            )
            .map_err(ctx("touch thumbnail"))?;
        }
        Ok(png)
    }

    /// Total bytes of cached thumbnails.
    pub fn thumbnail_bytes(&self) -> Result<u64> {
        self.lock()
            .query_row(
                "SELECT COALESCE(SUM(bytes), 0) FROM thumbnails",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|total| total as u64)
            .map_err(ctx("measure thumbnails"))
    }

    /// Drop the least recently used thumbnails until the cache fits `max_bytes`.
    /// Returns how many rows were removed.
    pub fn prune_thumbnails(&self, max_bytes: u64) -> Result<usize> {
        let conn = self.lock();
        let mut total: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(bytes), 0) FROM thumbnails",
                [],
                |row| row.get(0),
            )
            .map_err(ctx("measure thumbnails"))?;
        let mut removed = 0;
        if total <= max_bytes as i64 {
            return Ok(0);
        }
        let mut statement = conn
            .prepare("SELECT key, bytes FROM thumbnails ORDER BY last_used ASC, rowid ASC")
            .map_err(ctx("prune thumbnails"))?;
        let victims: Vec<(String, i64)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(ctx("prune thumbnails"))?
            .collect::<std::result::Result<_, _>>()
            .map_err(ctx("prune thumbnails"))?;
        for (key, bytes) in victims {
            if total <= max_bytes as i64 {
                break;
            }
            conn.execute("DELETE FROM thumbnails WHERE key = ?1", [&key])
                .map_err(ctx("prune thumbnails"))?;
            total -= bytes;
            removed += 1;
        }
        Ok(removed)
    }
}

fn is_corruption(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::NotADatabase) | Some(rusqlite::ErrorCode::DatabaseCorrupt)
    )
}

fn migrate(conn: &Connection) -> Result<()> {
    let applied: usize = conn
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map(|v| v as usize)
        .map_err(ctx("read schema version"))?;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        // One transaction per step: a failed upgrade leaves the old schema intact.
        conn.execute_batch(&format!(
            "BEGIN; {sql} PRAGMA user_version = {}; COMMIT;",
            index + 1
        ))
        .map_err(ctx("migrate schema"))?;
    }
    Ok(())
}

/// Schema version this build expects.
pub fn schema_version() -> usize {
    MIGRATIONS.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, thread};

    fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state").join("veloce.db")).unwrap();
        (dir, store)
    }

    fn prefs(view: &str) -> FolderPrefs {
        FolderPrefs {
            view: Some(view.to_string()),
            sort_key: Some("Size".to_string()),
            sort_desc: true,
            dirs_first: false,
        }
    }

    #[test]
    fn opening_creates_missing_directories_and_the_schema() {
        let (dir, _store) = temp_store();
        assert!(dir.path().join("state").join("veloce.db").is_file());
    }

    #[test]
    fn settings_round_trip_and_overwrite() {
        let (_dir, store) = temp_store();
        assert_eq!(store.get_setting("sidebar_width").unwrap(), None);
        store.set_setting("sidebar_width", "200").unwrap();
        store.set_setting("sidebar_width", "230").unwrap();
        assert_eq!(
            store.get_setting("sidebar_width").unwrap().as_deref(),
            Some("230")
        );
    }

    #[test]
    fn a_batch_of_settings_lands_together() {
        let (_dir, store) = temp_store();
        store
            .set_settings(&[("a", "1".to_string()), ("b", "2".to_string())])
            .unwrap();
        assert_eq!(store.get_setting("a").unwrap().as_deref(), Some("1"));
        assert_eq!(store.get_setting("b").unwrap().as_deref(), Some("2"));
    }

    #[test]
    fn folder_prefs_round_trip_including_optional_fields() {
        let (_dir, store) = temp_store();
        store.set_folder_prefs("/a", &prefs("icons")).unwrap();
        store
            .set_folder_prefs(
                "/b",
                &FolderPrefs {
                    view: None,
                    sort_key: None,
                    sort_desc: false,
                    dirs_first: true,
                },
            )
            .unwrap();
        let mut all = store.all_folder_prefs().unwrap();
        all.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(all[0], ("/a".to_string(), prefs("icons")));
        assert_eq!(all[1].1.view, None);
        assert!(all[1].1.dirs_first);
    }

    #[test]
    fn saving_folder_prefs_again_replaces_the_row() {
        let (_dir, store) = temp_store();
        store.set_folder_prefs("/a", &prefs("icons")).unwrap();
        store.set_folder_prefs("/a", &prefs("columns")).unwrap();
        let all = store.all_folder_prefs().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].1.view.as_deref(), Some("columns"));
    }

    #[test]
    fn tabs_keep_their_order_and_active_flag_and_replace_per_pane() {
        let (_dir, store) = temp_store();
        let tab = |path: &str, active| TabState {
            path: path.to_string(),
            active,
        };
        store
            .save_tabs(0, &[tab("/a", false), tab("/b", true), tab("/c", false)])
            .unwrap();
        store.save_tabs(1, &[tab("/z", true)]).unwrap();
        assert_eq!(
            store.load_tabs(0).unwrap(),
            [tab("/a", false), tab("/b", true), tab("/c", false)]
        );
        store.save_tabs(0, &[tab("/only", true)]).unwrap();
        assert_eq!(store.load_tabs(0).unwrap(), [tab("/only", true)]);
        assert_eq!(store.load_tabs(1).unwrap(), [tab("/z", true)]);
        assert!(store.load_tabs(7).unwrap().is_empty());
    }

    #[test]
    fn recents_are_most_recent_first_and_count_visits() {
        let (_dir, store) = temp_store();
        store.record_visit_at("/a", 10).unwrap();
        store.record_visit_at("/b", 20).unwrap();
        store.record_visit_at("/a", 30).unwrap();
        assert_eq!(store.recent_paths(10).unwrap(), ["/a", "/b"]);
        assert_eq!(store.visit_count("/a").unwrap(), 2);
        assert_eq!(store.visit_count("/missing").unwrap(), 0);
        assert_eq!(store.recent_paths(1).unwrap(), ["/a"]);
    }

    #[test]
    fn recents_are_capped() {
        let (_dir, store) = temp_store();
        for n in 0..(MAX_RECENTS as i64 + 25) {
            store.record_visit_at(&format!("/p{n}"), n).unwrap();
        }
        let all = store.recent_paths(1000).unwrap();
        assert_eq!(all.len(), MAX_RECENTS);
        assert_eq!(all[0], format!("/p{}", MAX_RECENTS + 24));
        assert!(!all.contains(&"/p0".to_string()), "oldest were dropped");
    }

    #[test]
    fn thumbnails_round_trip_binary_data() {
        let (_dir, store) = temp_store();
        let png = vec![0u8, 255, 1, 2, 3, 0, 9];
        assert_eq!(store.get_thumbnail("k").unwrap(), None);
        store.put_thumbnail("k", &png).unwrap();
        assert_eq!(store.get_thumbnail("k").unwrap(), Some(png));
        assert_eq!(store.thumbnail_bytes().unwrap(), 7);
    }

    #[test]
    fn pruning_removes_the_least_recently_used_first() {
        let (_dir, store) = temp_store();
        store.put_thumbnail_at("old", &[0; 100], 1).unwrap();
        store.put_thumbnail_at("mid", &[0; 100], 2).unwrap();
        store.put_thumbnail_at("new", &[0; 100], 3).unwrap();
        // Reading "old" makes it the freshest.
        store.get_thumbnail_at("old", 4).unwrap();
        let removed = store.prune_thumbnails(200).unwrap();
        assert_eq!(removed, 1);
        assert!(store.get_thumbnail("mid").unwrap().is_none());
        assert!(store.get_thumbnail("old").unwrap().is_some());
        assert!(store.get_thumbnail("new").unwrap().is_some());
        assert!(store.thumbnail_bytes().unwrap() <= 200);
    }

    #[test]
    fn pruning_a_cache_that_fits_removes_nothing() {
        let (_dir, store) = temp_store();
        store.put_thumbnail("a", &[0; 10]).unwrap();
        assert_eq!(store.prune_thumbnails(1000).unwrap(), 0);
        assert_eq!(store.prune_thumbnails(0).unwrap(), 1);
    }

    #[test]
    fn state_survives_reopening_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("veloce.db");
        {
            let store = Store::open(&path).unwrap();
            store.set_setting("k", "v").unwrap();
            store.set_folder_prefs("/a", &prefs("list")).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.get_setting("k").unwrap().as_deref(), Some("v"));
        assert_eq!(store.all_folder_prefs().unwrap().len(), 1);
    }

    #[test]
    fn migrations_are_recorded_and_reopening_does_not_rerun_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("veloce.db");
        Store::open(&path).unwrap();
        // Would fail with "table settings already exists" if it re-ran.
        let store = Store::open(&path).unwrap();
        let version: i64 = store
            .lock()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version as usize, schema_version());
    }

    #[test]
    fn a_file_that_is_not_a_database_is_moved_aside_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("veloce.db");
        std::fs::write(
            &path,
            b"this is definitely not sqlite, just some text padding",
        )
        .unwrap();
        let store = Store::open(&path).unwrap();
        store.set_setting("k", "v").unwrap();
        assert_eq!(store.get_setting("k").unwrap().as_deref(), Some("v"));
        assert!(dir.path().join("veloce.db.corrupt").is_file());
    }

    #[test]
    fn opening_somewhere_unwritable_is_a_contextual_error() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"x").unwrap();
        // A directory cannot be created underneath a regular file.
        let error = Store::open(&blocker.join("sub").join("veloce.db"))
            .err()
            .expect("must fail");
        assert!(error.to_string().contains("create state directory"));
    }

    #[test]
    fn many_threads_can_write_through_one_store() {
        let (_dir, store) = temp_store();
        let store = Arc::new(store);
        let handles: Vec<_> = (0..4)
            .map(|worker| {
                let store = store.clone();
                thread::spawn(move || {
                    for n in 0..25 {
                        store.set_setting(&format!("w{worker}-{n}"), "x").unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(store.get_setting("w3-24").unwrap().as_deref(), Some("x"));
        assert_eq!(store.get_setting("w0-0").unwrap().as_deref(), Some("x"));
    }

    #[test]
    fn in_memory_stores_are_independent_and_fully_functional() {
        let a = Store::in_memory().unwrap();
        let b = Store::in_memory().unwrap();
        a.set_setting("k", "1").unwrap();
        assert_eq!(b.get_setting("k").unwrap(), None);
    }

    #[test]
    fn the_default_path_names_the_app() {
        if let Some(path) = Store::default_path() {
            assert!(path.ends_with("Veloce/veloce.db"));
        }
    }
}
