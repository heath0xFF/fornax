use crate::config::ConversationSettings;
use rusqlite::{Connection, params};

#[derive(Clone)]
pub struct Preset {
    pub id: i64,
    pub name: String,
    pub settings: ConversationSettings,
}

pub struct PresetStorage {
    conn: Connection,
}

impl Default for PresetStorage {
    fn default() -> Self {
        let conn = Connection::open(Self::db_path()).expect("Failed to open database");
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .ok();
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        Self { conn }
    }
}

impl PresetStorage {
    fn db_path() -> std::path::PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("fornax")
            .join("fornax.db")
    }

    pub fn list_presets(&self) -> Vec<Preset> {
        let mut stmt = match self.conn.prepare(
            "SELECT id, name, model, system_prompt, temperature, max_tokens, use_max_tokens,
                    top_p, frequency_penalty, presence_penalty, stop_sequences, endpoint
             FROM presets ORDER BY name COLLATE NOCASE",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map([], |row| {
            let stop_json: Option<String> = row.get(10).ok();
            let stop_sequences = stop_json
                .as_deref()
                .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                .unwrap_or_default();
            Ok(Preset {
                id: row.get(0)?,
                name: row.get(1)?,
                settings: ConversationSettings {
                    model: row.get(2).ok(),
                    system_prompt: row.get(3).ok(),
                    temperature: row.get::<_, Option<f64>>(4).ok().flatten().map(|v| v as f32),
                    max_tokens: row.get::<_, Option<i64>>(5).ok().flatten().map(|v| v as u32),
                    use_max_tokens: row.get::<_, i64>(6).unwrap_or(0) != 0,
                    top_p: row.get::<_, Option<f64>>(7).ok().flatten().map(|v| v as f32),
                    frequency_penalty: row
                        .get::<_, Option<f64>>(8)
                        .ok()
                        .flatten()
                        .map(|v| v as f32),
                    presence_penalty: row
                        .get::<_, Option<f64>>(9)
                        .ok()
                        .flatten()
                        .map(|v| v as f32),
                    stop_sequences,
                    endpoint: row.get(11).ok(),
                    working_dir: None,
                    auto_compact: None,
                    compact_threshold_pct: None,
                    compact_keep_recent: None,
                },
            })
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn create_preset(&self, name: &str, s: &ConversationSettings) -> Result<i64, String> {
        let stop_json = if s.stop_sequences.is_empty() {
            None
        } else {
            serde_json::to_string(&s.stop_sequences).ok()
        };
        self.conn
            .execute(
                "INSERT INTO presets
                 (name, model, system_prompt, temperature, max_tokens, use_max_tokens,
                  top_p, frequency_penalty, presence_penalty, stop_sequences, endpoint)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    name,
                    s.model,
                    s.system_prompt,
                    s.temperature.map(|v| v as f64),
                    s.max_tokens.map(|v| v as i64),
                    s.use_max_tokens as i64,
                    s.top_p.map(|v| v as f64),
                    s.frequency_penalty.map(|v| v as f64),
                    s.presence_penalty.map(|v| v as f64),
                    stop_json,
                    s.endpoint,
                ],
            )
            .map_err(|e| format!("Failed to create preset: {e}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_preset(&self, id: i64) {
        self.conn
            .execute("DELETE FROM presets WHERE id = ?1", params![id])
            .ok();
    }

    #[cfg(test)]
    pub(crate) fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("open :memory:");
        conn.execute_batch("PRAGMA foreign_keys=ON;").ok();
        conn.execute_batch(
            "CREATE TABLE presets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                model TEXT,
                system_prompt TEXT,
                temperature REAL,
                max_tokens INTEGER,
                use_max_tokens INTEGER NOT NULL DEFAULT 0,
                top_p REAL,
                frequency_penalty REAL,
                presence_penalty REAL,
                stop_sequences TEXT,
                endpoint TEXT
            );",
        )
        .expect("create presets table");
        Self { conn }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> PresetStorage {
        PresetStorage::new_in_memory()
    }

    #[test]
    fn presets_crud() {
        let s = mem();
        assert!(s.list_presets().is_empty());
        let settings = ConversationSettings {
            model: Some("haiku".to_string()),
            system_prompt: Some("be concise".to_string()),
            temperature: Some(0.7),
            ..ConversationSettings::default()
        };
        let pid = s.create_preset("Concise", &settings).unwrap();
        let listed = s.list_presets();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Concise");
        assert_eq!(listed[0].settings.model.as_deref(), Some("haiku"));
        s.delete_preset(pid);
        assert!(s.list_presets().is_empty());
    }
}