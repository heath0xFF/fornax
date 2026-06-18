use rusqlite::{Connection, params};

/// A project groups conversations in the sidebar.
pub struct Project {
    pub id: i64,
    pub name: String,
    pub pinned: bool,
}

pub struct ProjectStorage {
    conn: Connection,
}

impl Default for ProjectStorage {
    fn default() -> Self {
        let conn = Connection::open(Self::db_path()).expect("Failed to open database");
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .ok();
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        Self { conn }
    }
}

impl ProjectStorage {
    fn db_path() -> std::path::PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("fornax")
            .join("fornax.db")
    }

    pub fn create_project(&self, name: &str) -> Result<i64, String> {
        self.conn
            .execute("INSERT INTO projects (name) VALUES (?1)", params![name])
            .map_err(|e| format!("Failed to create project: {e}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_projects(&self) -> Vec<Project> {
        let mut stmt = match self.conn.prepare(
            "SELECT id, name, pinned FROM projects ORDER BY pinned DESC, name COLLATE NOCASE",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                pinned: row.get::<_, i64>(2)? != 0,
            })
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn rename_project(&self, id: i64, name: &str) {
        self.conn
            .execute("UPDATE projects SET name = ?1 WHERE id = ?2", params![name, id])
            .ok();
    }

    pub fn set_project_pinned(&self, id: i64, pinned: bool) {
        self.conn
            .execute(
                "UPDATE projects SET pinned = ?1 WHERE id = ?2",
                params![pinned as i64, id],
            )
            .ok();
    }

    /// Delete a project; its conversations are detached (project_id → NULL),
    /// not deleted.
    pub fn delete_project(&self, id: i64) {
        if let Ok(tx) = self.conn.unchecked_transaction() {
            tx.execute(
                "UPDATE conversations SET project_id = NULL WHERE project_id = ?1",
                params![id],
            )
            .ok();
            tx.execute("DELETE FROM projects WHERE id = ?1", params![id]).ok();
            tx.commit().ok();
        }
    }

    /// Assign (or clear, with `None`) a conversation's project.
    pub fn set_conversation_project(&self, conversation_id: i64, project_id: Option<i64>) {
        self.conn
            .execute(
                "UPDATE conversations SET project_id = ?1 WHERE id = ?2",
                params![project_id, conversation_id],
            )
            .ok();
    }

    #[cfg(test)]
    pub(crate) fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("open :memory:");
        conn.execute_batch("PRAGMA foreign_keys=ON;").ok();
        conn.execute_batch(
            "CREATE TABLE conversations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                project_id INTEGER
            );
            CREATE TABLE projects (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("create tables");
        Self { conn }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> ProjectStorage {
        ProjectStorage::new_in_memory()
    }

    #[test]
    fn projects_crud_and_membership() {
        let s = mem();
        assert!(s.list_projects().is_empty());
        let pid = s.create_project("Spark").unwrap();
        // Create a conversation (via the conversations table directly).
        s.conn
            .execute("INSERT INTO conversations (title) VALUES ('chat 1')", [])
            .unwrap();
        let cid = s.conn.last_insert_rowid();
        // Verify the conversation has no project.
        let proj_id: Option<i64> = s.conn
            .query_row("SELECT project_id FROM conversations WHERE id = ?1", [cid], |r| r.get(0))
            .unwrap();
        assert_eq!(proj_id, None);
        s.set_conversation_project(cid, Some(pid));
        let proj_id: Option<i64> = s.conn
            .query_row("SELECT project_id FROM conversations WHERE id = ?1", [cid], |r| r.get(0))
            .unwrap();
        assert_eq!(proj_id, Some(pid));
        s.set_project_pinned(pid, true);
        s.rename_project(pid, "Spark2");
        let projects = s.list_projects();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "Spark2");
        assert!(projects[0].pinned);
        s.delete_project(pid);
        assert!(s.list_projects().is_empty());
        // Conversation still exists, just detached.
        let proj_id: Option<i64> = s.conn
            .query_row("SELECT project_id FROM conversations WHERE id = ?1", [cid], |r| r.get(0))
            .unwrap();
        assert_eq!(proj_id, None);
    }
}