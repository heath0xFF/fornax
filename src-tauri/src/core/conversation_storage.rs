use crate::message::{Message, Role};
use rusqlite::{Connection, params};
use std::path::PathBuf;
use std::time::Duration;

/// Returned by `list_conversations`: enough to render the sidebar (pinned-first
/// sort, optional pin-marker glyph) without a second query per row.
pub struct Conversation {
    pub id: i64,
    pub title: String,
    pub pinned: bool,
    pub project_id: Option<i64>,
    pub endpoint: Option<String>,
}

/// One completed stream turn's measured usage, ready to persist.
pub struct UsageRecord<'a> {
    pub endpoint: &'a str,
    pub model: &'a str,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub ttft_ms: Option<f64>,
    pub decode_tok_s: Option<f64>,
    pub cost: Option<f64>,
    pub ok: bool,
}

pub struct ConversationStorage {
    conn: Connection,
}

impl Default for ConversationStorage {
    fn default() -> Self {
        let path = Self::db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        let conn = Connection::open(&path).expect("Failed to open database");
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .ok();
        let _ = conn.busy_timeout(Duration::from_secs(5));
        let storage = Self { conn };
        storage.init_tables();
        storage
    }
}

impl ConversationStorage {
    fn db_path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fornax")
            .join("fornax.db")
    }

    fn init_tables(&self) {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_version (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE TABLE IF NOT EXISTS conversations (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    title TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE TABLE IF NOT EXISTS messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    conversation_id INTEGER NOT NULL,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS presets (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE TABLE IF NOT EXISTS projects (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    pinned INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE TABLE IF NOT EXISTS usage (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts TEXT NOT NULL DEFAULT (datetime('now')),
                    endpoint TEXT NOT NULL DEFAULT '',
                    model TEXT NOT NULL DEFAULT '',
                    prompt_tokens INTEGER NOT NULL DEFAULT 0,
                    completion_tokens INTEGER NOT NULL DEFAULT 0,
                    ttft_ms REAL,
                    decode_tok_s REAL,
                    cost REAL,
                    ok INTEGER NOT NULL DEFAULT 1
                );",
            )
            .expect("Failed to create baseline tables");

        self.ensure_v2_columns();

        self.conn
            .execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_messages_conv_position ON messages(conversation_id, position);
                 CREATE INDEX IF NOT EXISTS idx_messages_parent ON messages(parent_id);
                 CREATE INDEX IF NOT EXISTS idx_conversations_pinned_updated ON conversations(pinned DESC, updated_at DESC);
                 CREATE INDEX IF NOT EXISTS idx_usage_ts ON usage(ts);
                 INSERT OR IGNORE INTO schema_version (version) VALUES (2);",
            )
            .expect("Failed to create indexes");
    }

    /// Bring every table up to the v2 column set by ALTERing in any column
    /// that's missing.
    fn ensure_v2_columns(&self) {
        const CONV_COLS: &[(&str, &str)] = &[
            ("model", "TEXT"),
            ("system_prompt", "TEXT"),
            ("temperature", "REAL"),
            ("max_tokens", "INTEGER"),
            ("use_max_tokens", "INTEGER NOT NULL DEFAULT 0"),
            ("top_p", "REAL"),
            ("frequency_penalty", "REAL"),
            ("presence_penalty", "REAL"),
            ("stop_sequences", "TEXT"),
            ("endpoint", "TEXT"),
            ("pinned", "INTEGER NOT NULL DEFAULT 0"),
            ("draft", "TEXT"),
            ("auto_titled", "INTEGER NOT NULL DEFAULT 0"),
            ("working_dir", "TEXT"),
            ("project_id", "INTEGER"),
            ("auto_compact", "INTEGER"),
            ("compact_threshold_pct", "REAL"),
            ("compact_keep_recent", "INTEGER"),
            ("summary", "TEXT"),
            ("summary_through", "INTEGER"),
        ];
        const MSG_COLS: &[(&str, &str)] = &[
            ("parent_id", "INTEGER REFERENCES messages(id) ON DELETE CASCADE"),
            ("branch_index", "INTEGER NOT NULL DEFAULT 0"),
            ("created_at", "INTEGER"),
            ("tool_calls", "TEXT"),
            ("tool_call_id", "TEXT"),
        ];
        const PRESET_COLS: &[(&str, &str)] = &[
            ("model", "TEXT"),
            ("system_prompt", "TEXT"),
            ("temperature", "REAL"),
            ("max_tokens", "INTEGER"),
            ("use_max_tokens", "INTEGER NOT NULL DEFAULT 0"),
            ("top_p", "REAL"),
            ("frequency_penalty", "REAL"),
            ("presence_penalty", "REAL"),
            ("stop_sequences", "TEXT"),
            ("endpoint", "TEXT"),
        ];

        const USAGE_COLS: &[(&str, &str)] = &[("cost", "REAL")];

        for (col, ty) in CONV_COLS {
            self.add_column_if_missing("conversations", col, ty);
        }
        for (col, ty) in MSG_COLS {
            self.add_column_if_missing("messages", col, ty);
        }
        for (col, ty) in PRESET_COLS {
            self.add_column_if_missing("presets", col, ty);
        }
        for (col, ty) in USAGE_COLS {
            self.add_column_if_missing("usage", col, ty);
        }
    }

    fn add_column_if_missing(&self, table: &str, column: &str, type_def: &str) {
        let exists: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                params![table, column],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if exists == 0 {
            let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {type_def}");
            if let Err(e) = self.conn.execute(&sql, []) {
                eprintln!("Migration warning: failed to add {table}.{column}: {e}");
            }
        }
    }

    pub fn create_conversation(&self, title: &str) -> Result<i64, String> {
        self.conn
            .execute(
                "INSERT INTO conversations (title) VALUES (?1)",
                params![title],
            )
            .map_err(|e| format!("Failed to create conversation: {e}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Record one completed stream turn's usage. Called for every turn (the
    /// tool loop records each continuation), so totals reflect real token spend.
    pub fn record_usage(&self, r: &UsageRecord) {
        self.conn
            .execute(
                "INSERT INTO usage \
                 (endpoint, model, prompt_tokens, completion_tokens, ttft_ms, decode_tok_s, cost, ok) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    r.endpoint,
                    r.model,
                    r.prompt_tokens.unwrap_or(0),
                    r.completion_tokens.unwrap_or(0),
                    r.ttft_ms,
                    r.decode_tok_s,
                    r.cost,
                    r.ok as i64,
                ],
            )
            .ok();
    }

    pub fn update_conversation_title(&self, id: i64, title: &str) {
        self.conn
            .execute(
                "UPDATE conversations SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![title, id],
            )
            .ok();
    }

    pub fn list_conversations(&self) -> Vec<Conversation> {
        let mut stmt = match self.conn.prepare(
            "SELECT id, title, pinned, project_id, endpoint FROM conversations \
             ORDER BY pinned DESC, updated_at DESC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        match stmt.query_map([], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                pinned: row.get::<_, i64>(2)? != 0,
                project_id: row.get(3).ok(),
                endpoint: row.get(4).ok(),
            })
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn delete_conversation(&self, id: i64) {
        if let Ok(tx) = self.conn.unchecked_transaction() {
            let r1 = tx.execute(
                "DELETE FROM messages WHERE conversation_id = ?1",
                params![id],
            );
            let r2 = tx.execute("DELETE FROM conversations WHERE id = ?1", params![id]);

            if r1.is_ok() && r2.is_ok() {
                tx.commit().ok();
            }
        }
    }

    pub fn delete_all_conversations(&self) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start transaction: {e}"))?;
        tx.execute("DELETE FROM messages", [])
            .map_err(|e| format!("Failed to delete messages: {e}"))?;
        tx.execute("DELETE FROM conversations", [])
            .map_err(|e| format!("Failed to delete conversations: {e}"))?;
        tx.commit().map_err(|e| format!("Failed to commit: {e}"))
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) {
        self.conn
            .execute(
                "UPDATE conversations SET pinned = ?1 WHERE id = ?2",
                params![pinned as i64, id],
            )
            .ok();
    }

    /// Incremental save: messages with `id = None` are INSERTed (and have
    /// their assigned rowid written back into the struct via the caller's
    /// `&mut`); messages with `id = Some` are UPDATEd in place.
    pub fn save_messages(
        &self,
        conversation_id: i64,
        messages: &mut [Message],
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start transaction: {e}"))?;

        let mut next_position: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position), -1) FROM messages WHERE conversation_id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .unwrap_or(-1)
            + 1;

        let mut prev_id: Option<i64> = None;
        for msg in messages.iter_mut() {
            let role_str = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            let content_json = serde_json::to_string(&msg.content)
                .map_err(|e| format!("Failed to encode content: {e}"))?;
            let tool_calls_json: Option<String> = match &msg.tool_calls {
                Some(tcs) if !tcs.is_empty() => Some(
                    serde_json::to_string(tcs)
                        .map_err(|e| format!("Failed to encode tool_calls: {e}"))?,
                ),
                _ => None,
            };
            match msg.id {
                Some(id) => {
                    tx.execute(
                        "UPDATE messages SET content = ?1, created_at = ?2,
                                             tool_calls = ?3, tool_call_id = ?4
                         WHERE id = ?5",
                        params![
                            content_json,
                            msg.created_at,
                            tool_calls_json,
                            msg.tool_call_id,
                            id
                        ],
                    )
                    .map_err(|e| format!("Failed to update message: {e}"))?;
                    prev_id = Some(id);
                }
                None => {
                    if msg.parent_id.is_none() {
                        msg.parent_id = prev_id.or_else(|| {
                            tx.query_row(
                                "SELECT id FROM messages
                                 WHERE conversation_id = ?1
                                 ORDER BY position DESC LIMIT 1",
                                params![conversation_id],
                                |row| row.get::<_, i64>(0),
                            )
                            .ok()
                        });
                    }
                    tx.execute(
                        "INSERT INTO messages
                         (conversation_id, role, content, position, parent_id, branch_index,
                          created_at, tool_calls, tool_call_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![
                            conversation_id,
                            role_str,
                            content_json,
                            next_position,
                            msg.parent_id,
                            msg.branch_index,
                            msg.created_at,
                            tool_calls_json,
                            msg.tool_call_id,
                        ],
                    )
                    .map_err(|e| format!("Failed to insert message: {e}"))?;
                    let new_id = tx.last_insert_rowid();
                    msg.id = Some(new_id);
                    prev_id = Some(new_id);
                    next_position += 1;
                }
            }
        }

        tx.execute(
            "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
            params![conversation_id],
        )
        .map_err(|e| format!("Failed to update timestamp: {e}"))?;

        tx.commit().map_err(|e| format!("Failed to commit: {e}"))
    }

    pub fn load_messages(&self, conversation_id: i64) -> Vec<Message> {
        let rows = match self.fetch_all_message_rows(conversation_id) {
            Some(r) => r,
            None => return Vec::new(),
        };
        if rows.is_empty() {
            return Vec::new();
        }

        let any_parent_set = rows.iter().any(|r| r.parent_id.is_some());
        if !any_parent_set {
            let mut sorted = rows;
            sorted.sort_by_key(|r| r.position);
            return sorted.into_iter().map(MessageRow::into_message).collect();
        }

        use std::collections::HashMap;
        let mut by_parent: HashMap<Option<i64>, Vec<MessageRow>> = HashMap::new();
        for r in rows {
            by_parent.entry(r.parent_id).or_default().push(r);
        }
        for bucket in by_parent.values_mut() {
            bucket.sort_by_key(|r| (std::cmp::Reverse(r.created_at), std::cmp::Reverse(r.branch_index)));
        }

        let mut out: Vec<Message> = Vec::new();
        let mut current_parent: Option<i64> = None;
        while let Some(bucket) = by_parent.get(&current_parent) {
            let Some(picked) = bucket.first() else {
                break;
            };
            let id = picked.id;
            out.push(picked.clone().into_message());
            current_parent = Some(id);
        }
        out
    }

    fn fetch_all_message_rows(&self, conversation_id: i64) -> Option<Vec<MessageRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, role, content, created_at, parent_id, branch_index, position,
                        tool_calls, tool_call_id
                 FROM messages WHERE conversation_id = ?1",
            )
            .ok()?;
        let mapped = stmt
            .query_map(params![conversation_id], MessageRow::from_row)
            .ok()?;
        Some(mapped.filter_map(|r| r.ok()).collect())
    }

    /// One-time backfill: set `parent_id` of each row to the id of the
    /// previous row (by position) within the same conversation.
    pub fn backfill_parent_ids(&self, conversation_id: i64) -> Result<(), String> {
        let already: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM messages
                 WHERE conversation_id = ?1 AND parent_id IS NOT NULL",
                params![conversation_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if already > 0 {
            return Ok(());
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start backfill tx: {e}"))?;
        let pairs: Vec<(i64, i64)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, position FROM messages
                     WHERE conversation_id = ?1 ORDER BY position",
                )
                .map_err(|e| format!("Failed to prepare backfill query: {e}"))?;
            stmt.query_map(params![conversation_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("Failed to query backfill rows: {e}"))?
            .filter_map(|r| r.ok())
            .collect()
        };
        let mut prev_id: Option<i64> = None;
        for (id, _pos) in pairs {
            if let Some(p) = prev_id {
                tx.execute(
                    "UPDATE messages SET parent_id = ?1 WHERE id = ?2",
                    params![p, id],
                )
                .map_err(|e| format!("Failed backfill update: {e}"))?;
            }
            prev_id = Some(id);
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit backfill: {e}"))
    }

    pub fn siblings_of(&self, message_id: i64) -> Vec<MessageHeader> {
        let row: Option<(Option<i64>, i64)> = self
            .conn
            .query_row(
                "SELECT parent_id, conversation_id FROM messages WHERE id = ?1",
                params![message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        let Some((parent_id, conv_id)) = row else {
            return Vec::new();
        };
        let mut stmt = match parent_id {
            Some(_) => self.conn.prepare(
                "SELECT id, branch_index, created_at FROM messages
                 WHERE conversation_id = ?1 AND parent_id = ?2
                 ORDER BY branch_index",
            ),
            None => self.conn.prepare(
                "SELECT id, branch_index, created_at FROM messages
                 WHERE conversation_id = ?1 AND parent_id IS NULL
                 ORDER BY branch_index",
            ),
        };
        let Ok(stmt) = &mut stmt else {
            return Vec::new();
        };
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok::<MessageHeader, rusqlite::Error>(MessageHeader {
                id: row.get(0)?,
                branch_index: row.get(1)?,
                created_at: row.get(2).ok(),
            })
        };
        match parent_id {
            Some(pid) => stmt
                .query_map(params![conv_id, pid], map_row)
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default(),
            None => stmt
                .query_map(params![conv_id], map_row)
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default(),
        }
    }

    pub fn walk_from(&self, start_id: i64) -> Vec<Message> {
        let conv_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT conversation_id FROM messages WHERE id = ?1",
                params![start_id],
                |row| row.get(0),
            )
            .ok();
        let Some(conv_id) = conv_id else {
            return Vec::new();
        };
        let rows = match self.fetch_all_message_rows(conv_id) {
            Some(r) => r,
            None => return Vec::new(),
        };
        let start = match rows.iter().find(|r| r.id == start_id) {
            Some(r) => r.clone(),
            None => return Vec::new(),
        };

        use std::collections::HashMap;
        let mut by_parent: HashMap<Option<i64>, Vec<MessageRow>> = HashMap::new();
        for r in rows {
            by_parent.entry(r.parent_id).or_default().push(r);
        }
        for bucket in by_parent.values_mut() {
            bucket.sort_by_key(|r| (std::cmp::Reverse(r.created_at), std::cmp::Reverse(r.branch_index)));
        }

        let mut out = vec![start.clone().into_message()];
        let mut current_parent: Option<i64> = Some(start.id);
        while let Some(bucket) = by_parent.get(&current_parent) {
            let Some(picked) = bucket.first() else {
                break;
            };
            let id = picked.id;
            out.push(picked.clone().into_message());
            current_parent = Some(id);
        }
        out
    }

    pub fn search(&self, query: &str) -> Vec<(i64, String, String)> {
        let q_lower = query.to_ascii_lowercase();
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let mut stmt = match self.conn.prepare(
            "SELECT c.id, c.title, m.content FROM messages m
             JOIN conversations c ON c.id = m.conversation_id
             WHERE m.content LIKE ?1 ESCAPE '\\'
             ORDER BY c.updated_at DESC
             LIMIT 200",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let candidates: Vec<(i64, String, String)> = stmt
            .query_map(params![pattern], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        candidates
            .into_iter()
            .filter_map(|(id, title, raw)| {
                let parts: Vec<crate::message::ContentPart> =
                    serde_json::from_str(&raw).unwrap_or_else(|_| {
                        vec![crate::message::ContentPart::Text { text: raw.clone() }]
                    });
                let mut text = String::new();
                for p in &parts {
                    if let crate::message::ContentPart::Text { text: t } = p {
                        text.push_str(t);
                        text.push('\n');
                    }
                }
                if text.to_ascii_lowercase().contains(&q_lower) {
                    Some((id, title, text))
                } else {
                    None
                }
            })
            .take(50)
            .collect()
    }

    pub fn export_markdown(&self, conversation_id: i64) -> String {
        let messages = self.load_messages(conversation_id);
        let mut out = String::new();

        let title: String = self
            .conn
            .query_row(
                "SELECT title FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "Conversation".to_string());

        out.push_str(&format!("# {title}\n\n"));

        for msg in &messages {
            let role = match msg.role {
                crate::message::Role::User => "**You**",
                crate::message::Role::Assistant => "**AI**",
                crate::message::Role::System => "**System**",
                crate::message::Role::Tool => "**Tool**",
            };
            let body = msg.text_str();
            let image_count = msg.images().count();
            let suffix = if image_count > 0 {
                format!("\n\n_({image_count} image attachment(s))_")
            } else {
                String::new()
            };
            out.push_str(&format!("{role}:\n\n{body}{suffix}\n\n---\n\n"));
        }

        out
    }

    // ---- per-conversation settings ----

    pub fn load_conversation_settings(&self, id: i64) -> crate::config::ConversationSettings {
        self.conn
            .query_row(
                "SELECT model, system_prompt, temperature, max_tokens, use_max_tokens,
                        top_p, frequency_penalty, presence_penalty, stop_sequences, endpoint,
                        working_dir, auto_compact, compact_threshold_pct, compact_keep_recent
                 FROM conversations WHERE id = ?1",
                params![id],
                |row| {
                    let stop_json: Option<String> = row.get(8).ok();
                    let stop_sequences = stop_json
                        .as_deref()
                        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                        .unwrap_or_default();
                    Ok(crate::config::ConversationSettings {
                        model: row.get(0).ok(),
                        system_prompt: row.get(1).ok(),
                        temperature: row.get::<_, Option<f64>>(2).ok().flatten().map(|v| v as f32),
                        max_tokens: row.get::<_, Option<i64>>(3).ok().flatten().map(|v| v as u32),
                        use_max_tokens: row.get::<_, i64>(4).unwrap_or(0) != 0,
                        top_p: row.get::<_, Option<f64>>(5).ok().flatten().map(|v| v as f32),
                        frequency_penalty: row
                            .get::<_, Option<f64>>(6)
                            .ok()
                            .flatten()
                            .map(|v| v as f32),
                        presence_penalty: row
                            .get::<_, Option<f64>>(7)
                            .ok()
                            .flatten()
                            .map(|v| v as f32),
                        stop_sequences,
                        endpoint: row.get(9).ok(),
                        working_dir: row.get(10).ok(),
                        auto_compact: row
                            .get::<_, Option<i64>>(11)
                            .ok()
                            .flatten()
                            .map(|v| v != 0),
                        compact_threshold_pct: row
                            .get::<_, Option<f64>>(12)
                            .ok()
                            .flatten()
                            .map(|v| v as f32),
                        compact_keep_recent: row
                            .get::<_, Option<i64>>(13)
                            .ok()
                            .flatten()
                            .map(|v| v as u32),
                    })
                },
            )
            .unwrap_or_default()
    }

    pub fn save_conversation_settings(&self, id: i64, s: &crate::config::ConversationSettings) {
        let stop_json = if s.stop_sequences.is_empty() {
            None
        } else {
            serde_json::to_string(&s.stop_sequences).ok()
        };
        self.conn
            .execute(
                "UPDATE conversations
                 SET model = ?1, system_prompt = ?2, temperature = ?3, max_tokens = ?4,
                     use_max_tokens = ?5, top_p = ?6, frequency_penalty = ?7,
                     presence_penalty = ?8, stop_sequences = ?9, endpoint = ?10,
                     working_dir = ?11, auto_compact = ?12, compact_threshold_pct = ?13,
                     compact_keep_recent = ?14,
                     updated_at = datetime('now')
                 WHERE id = ?15",
                params![
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
                    s.working_dir,
                    s.auto_compact.map(|v| v as i64),
                    s.compact_threshold_pct.map(|v| v as f64),
                    s.compact_keep_recent.map(|v| v as i64),
                    id,
                ],
            )
            .ok();
    }

    /// The persisted compaction summary and the message id it covers up to
    /// (inclusive). `(None, None)` when the conversation has never been compacted.
    pub fn load_compaction(&self, id: i64) -> (Option<String>, Option<i64>) {
        self.conn
            .query_row(
                "SELECT summary, summary_through FROM conversations WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                    ))
                },
            )
            .unwrap_or((None, None))
    }

    pub fn save_compaction(&self, id: i64, summary: &str, through: i64) {
        self.conn
            .execute(
                "UPDATE conversations SET summary = ?1, summary_through = ?2 WHERE id = ?3",
                params![summary, through, id],
            )
            .ok();
    }

    pub fn save_draft(&self, id: i64, draft: &str) {
        self.conn
            .execute(
                "UPDATE conversations SET draft = ?1 WHERE id = ?2",
                params![draft, id],
            )
            .ok();
    }

    pub fn load_draft(&self, id: i64) -> Option<String> {
        self.conn
            .query_row(
                "SELECT draft FROM conversations WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
    }

    pub fn needs_auto_title(&self, id: i64) -> bool {
        self.conn
            .query_row(
                "SELECT auto_titled FROM conversations WHERE id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .map(|v| v == 0)
            .unwrap_or(false)
    }

    pub fn mark_auto_titled(&self, id: i64) {
        self.conn
            .execute(
                "UPDATE conversations SET auto_titled = 1 WHERE id = ?1",
                params![id],
            )
            .ok();
    }

    /// Next branch index for a new child message under `parent_id`.
    /// `parent_id = None` means root-level (no parent).
    pub fn next_branch_index(&self, conversation_id: i64, parent_id: Option<i64>) -> i64 {
        let max: i64 = match parent_id {
            Some(pid) => self
                .conn
                .query_row(
                    "SELECT COALESCE(MAX(branch_index), -1) FROM messages
                     WHERE conversation_id = ?1 AND parent_id = ?2",
                    params![conversation_id, pid],
                    |row| row.get(0),
                )
                .unwrap_or(-1),
            None => self
                .conn
                .query_row(
                    "SELECT COALESCE(MAX(branch_index), -1) FROM messages
                     WHERE conversation_id = ?1 AND parent_id IS NULL",
                    params![conversation_id],
                    |row| row.get(0),
                )
                .unwrap_or(-1),
        };
        max + 1
    }

    #[cfg(test)]
    pub(crate) fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("open :memory:");
        conn.execute_batch("PRAGMA foreign_keys=ON;").ok();
        let storage = Self { conn };
        storage.init_tables();
        storage
    }

    #[cfg(test)]
    pub(crate) fn new_legacy_0_6_with_data() -> Self {
        let conn = Connection::open_in_memory().expect("open :memory:");
        conn.execute_batch(
            "CREATE TABLE conversations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                position INTEGER NOT NULL,
                FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
            );",
        )
        .expect("create legacy schema");
        conn.execute("INSERT INTO conversations (title) VALUES ('legacy chat')", []).unwrap();
        conn.execute(
            "INSERT INTO messages (conversation_id, role, content, position)
             VALUES (1, 'user', 'hello from 0.6', 0)",
            [],
        )
        .unwrap();
        Self { conn }
    }
}

#[derive(Clone)]
pub struct MessageHeader {
    pub id: i64,
    #[allow(dead_code)]
    pub branch_index: i64,
    #[allow(dead_code)]
    pub created_at: Option<i64>,
}

#[derive(Clone)]
struct MessageRow {
    id: i64,
    role: crate::message::Role,
    content: Vec<crate::message::ContentPart>,
    created_at: Option<i64>,
    parent_id: Option<i64>,
    branch_index: i64,
    #[allow(dead_code)]
    position: i64,
    tool_calls: Option<Vec<crate::message::ToolCall>>,
    tool_call_id: Option<String>,
}

impl MessageRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let role_str: String = row.get(1)?;
        let role = match role_str.as_str() {
            "system" => crate::message::Role::System,
            "user" => crate::message::Role::User,
            "assistant" => crate::message::Role::Assistant,
            "tool" => crate::message::Role::Tool,
            _ => crate::message::Role::Assistant,
        };
        let content_raw: String = row.get(2)?;
        let content: Vec<crate::message::ContentPart> = serde_json::from_str(&content_raw)
            .unwrap_or_else(|_| vec![crate::message::ContentPart::Text { text: content_raw }]);
        let tool_calls_raw: Option<String> = row.get(7).ok();
        let tool_calls = tool_calls_raw
            .as_deref()
            .and_then(|s| serde_json::from_str::<Vec<crate::message::ToolCall>>(s).ok());
        Ok(MessageRow {
            id: row.get(0)?,
            role,
            content,
            created_at: row.get(3).ok(),
            parent_id: row.get(4).ok(),
            branch_index: row.get(5)?,
            position: row.get(6)?,
            tool_calls,
            tool_call_id: row.get(8).ok(),
        })
    }

    fn into_message(self) -> crate::message::Message {
        crate::message::Message {
            role: self.role,
            content: self.content,
            tool_calls: self.tool_calls,
            tool_call_id: self.tool_call_id,
            created_at: self.created_at,
            id: Some(self.id),
            parent_id: self.parent_id,
            branch_index: self.branch_index,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ContentPart, ImageUrl, Message, Role};

    fn mem() -> ConversationStorage {
        ConversationStorage::new_in_memory()
    }

    fn ts(base: i64, offset: i64) -> Option<i64> {
        Some(base + offset)
    }

    #[test]
    fn init_tables_migrates_legacy_0_6_db_in_place() {
        let s = ConversationStorage::new_legacy_0_6_with_data();
        s.init_tables();
        let convs = s.list_conversations();
        assert_eq!(convs.len(), 1);
        assert_eq!(convs[0].title, "legacy chat");
        assert!(!convs[0].pinned);
        let msgs = s.load_messages(convs[0].id);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text_str(), "hello from 0.6");
        assert_eq!(msgs[0].parent_id, None);
        assert_eq!(msgs[0].branch_index, 0);
        // Subsequent saves work into the migrated schema.
        let mut new_msgs = vec![Message::text(Role::Assistant, "hi from 0.8".into())];
        s.save_messages(convs[0].id, &mut new_msgs)
            .expect("save into migrated DB");
        assert!(new_msgs[0].id.is_some());
    }

    #[test]
    fn init_tables_is_idempotent() {
        let s = ConversationStorage::new_legacy_0_6_with_data();
        s.init_tables();
        s.init_tables();
        // Should not panic — running twice is a no-op.
    }

    #[test]
    fn round_trips_text_messages() {
        let s = mem();
        let id = s.create_conversation("hello").unwrap();
        let mut msgs = vec![
            Message::text(Role::User, "hi".to_string()),
            Message::text(Role::Assistant, "hello!".to_string()),
        ];
        s.save_messages(id, &mut msgs).unwrap();
        let loaded = s.load_messages(id);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].text_str(), "hi");
        assert_eq!(loaded[1].text_str(), "hello!");
        assert_eq!(loaded[1].parent_id, loaded[0].id);
    }

    #[test]
    fn round_trips_messages_with_images() {
        let s = mem();
        let id = s.create_conversation("vision").unwrap();
        let parts = vec![
            ContentPart::Text {
                text: "what's in this".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,AAAA".to_string(),
                    detail: None,
                },
            },
        ];
        let mut msgs = vec![Message::from_parts(Role::User, parts.clone())];
        s.save_messages(id, &mut msgs).unwrap();
        let loaded = s.load_messages(id);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].content, parts);
    }

    #[test]
    fn search_post_filters_json_keys() {
        let s = mem();
        let id = s.create_conversation("vision conv").unwrap();
        s.save_messages(
            id,
            &mut [Message::from_parts(
                Role::User,
                vec![
                    ContentPart::Text {
                        text: "hello world".to_string(),
                    },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "data:image/png;base64,AAAA".to_string(),
                            detail: None,
                        },
                    },
                ],
            )],
        )
        .unwrap();
        // "image_url" is a JSON key but not in any text part — must NOT match.
        assert!(s.search("image_url").is_empty());
        assert_eq!(s.search("hello world").len(), 1);
    }

    #[test]
    fn round_trips_per_conversation_settings() {
        let s = mem();
        let id = s.create_conversation("settings test").unwrap();
        let original = crate::config::ConversationSettings {
            model: Some("gpt-4o".to_string()),
            system_prompt: Some("be terse".to_string()),
            temperature: Some(0.42),
            max_tokens: Some(1234),
            use_max_tokens: true,
            top_p: Some(0.9),
            frequency_penalty: None,
            presence_penalty: Some(0.5),
            stop_sequences: vec!["END".to_string(), "STOP".to_string()],
            endpoint: Some("https://openrouter.ai/api/v1".to_string()),
            working_dir: Some("/tmp/work".to_string()),
            auto_compact: Some(false),
            compact_threshold_pct: Some(0.6),
            compact_keep_recent: Some(12),
        };
        s.save_conversation_settings(id, &original);
        let loaded = s.load_conversation_settings(id);
        assert_eq!(loaded.model, original.model);
        assert_eq!(loaded.system_prompt, original.system_prompt);
        assert_eq!(loaded.temperature, original.temperature);
        assert_eq!(loaded.max_tokens, original.max_tokens);
        assert_eq!(loaded.use_max_tokens, original.use_max_tokens);
        assert_eq!(loaded.top_p, original.top_p);
        assert_eq!(loaded.frequency_penalty, original.frequency_penalty);
        assert_eq!(loaded.presence_penalty, original.presence_penalty);
        assert_eq!(loaded.stop_sequences, original.stop_sequences);
        assert_eq!(loaded.endpoint, original.endpoint);
        assert_eq!(loaded.auto_compact, original.auto_compact);
        assert_eq!(loaded.compact_threshold_pct, original.compact_threshold_pct);
        assert_eq!(loaded.compact_keep_recent, original.compact_keep_recent);
    }

    #[test]
    fn round_trips_compaction_state() {
        let s = mem();
        let id = s.create_conversation("compaction test").unwrap();
        assert_eq!(s.load_compaction(id), (None, None));
        s.save_compaction(id, "earlier summary", 42);
        assert_eq!(
            s.load_compaction(id),
            (Some("earlier summary".to_string()), Some(42))
        );
    }

    #[test]
    fn pinned_conversations_sort_to_top() {
        let s = mem();
        let a = s.create_conversation("first").unwrap();
        let b = s.create_conversation("second").unwrap();
        let c = s.create_conversation("third").unwrap();
        s.set_pinned(a, true);
        let listed = s.list_conversations();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].id, a);
        assert!(listed[0].pinned);
        let rest: Vec<i64> = listed[1..].iter().map(|c| c.id).collect();
        assert!(rest.contains(&b));
        assert!(rest.contains(&c));
    }

    #[test]
    fn drafts_round_trip_and_clear() {
        let s = mem();
        let id = s.create_conversation("drafty").unwrap();
        s.save_draft(id, "in progress");
        assert_eq!(s.load_draft(id).as_deref(), Some("in progress"));
        s.save_draft(id, "");
        assert_eq!(s.load_draft(id), None);
    }

    #[test]
    fn auto_title_flag_is_one_shot() {
        let s = mem();
        let id = s.create_conversation("t").unwrap();
        assert!(s.needs_auto_title(id));
        s.mark_auto_titled(id);
        assert!(!s.needs_auto_title(id));
    }

    #[test]
    fn save_messages_assigns_ids_and_writes_them_back() {
        let s = mem();
        let conv = s.create_conversation("ids").unwrap();
        let mut msgs = vec![
            Message::text(Role::User, "u".into()),
            Message::text(Role::Assistant, "a".into()),
        ];
        s.save_messages(conv, &mut msgs).unwrap();
        assert!(msgs[0].id.is_some());
        assert!(msgs[1].id.is_some());
        assert_eq!(msgs[1].parent_id, msgs[0].id);
    }

    #[test]
    fn save_messages_updates_existing_in_place() {
        let s = mem();
        let conv = s.create_conversation("update").unwrap();
        let mut msgs = vec![Message::text(Role::User, "before".into())];
        s.save_messages(conv, &mut msgs).unwrap();
        let id = msgs[0].id.unwrap();
        msgs[0].content = vec![ContentPart::Text {
            text: "after".into(),
        }];
        s.save_messages(conv, &mut msgs).unwrap();
        assert_eq!(msgs[0].id, Some(id));
        let loaded = s.load_messages(conv);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].text_str(), "after");
    }

    #[test]
    fn legacy_load_falls_back_to_position_order() {
        let s = mem();
        let conv = s.create_conversation("legacy").unwrap();
        let mut msgs = vec![
            Message::text(Role::User, "u1".into()),
            Message::text(Role::Assistant, "a1".into()),
            Message::text(Role::User, "u2".into()),
            Message::text(Role::Assistant, "a2".into()),
        ];
        s.save_messages(conv, &mut msgs).unwrap();
        s.conn
            .execute(
                "UPDATE messages SET parent_id = NULL WHERE conversation_id = ?1",
                params![conv],
            )
            .unwrap();
        let loaded = s.load_messages(conv);
        assert_eq!(
            loaded.iter().map(|m| m.text_str()).collect::<Vec<_>>(),
            vec!["u1", "a1", "u2", "a2"]
        );
    }

    #[test]
    fn backfill_parent_ids_chains_legacy_rows() {
        let s = mem();
        let conv = s.create_conversation("backfill").unwrap();
        let mut msgs = vec![
            Message::text(Role::User, "u1".into()),
            Message::text(Role::Assistant, "a1".into()),
            Message::text(Role::User, "u2".into()),
        ];
        s.save_messages(conv, &mut msgs).unwrap();
        s.conn
            .execute(
                "UPDATE messages SET parent_id = NULL WHERE conversation_id = ?1",
                params![conv],
            )
            .unwrap();
        s.backfill_parent_ids(conv).unwrap();
        let loaded = s.load_messages(conv);
        assert_eq!(loaded[0].parent_id, None);
        assert_eq!(loaded[1].parent_id, loaded[0].id);
        assert_eq!(loaded[2].parent_id, loaded[1].id);
    }

    #[test]
    fn backfill_parent_ids_is_idempotent() {
        let s = mem();
        let conv = s.create_conversation("idempotent").unwrap();
        let mut msgs = vec![
            Message::text(Role::User, "u".into()),
            Message::text(Role::Assistant, "a".into()),
        ];
        s.save_messages(conv, &mut msgs).unwrap();
        let before = s.load_messages(conv);
        s.backfill_parent_ids(conv).unwrap();
        s.backfill_parent_ids(conv).unwrap();
        let after = s.load_messages(conv);
        assert_eq!(
            after.iter().map(|m| m.parent_id).collect::<Vec<_>>(),
            before.iter().map(|m| m.parent_id).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn branched_load_picks_newest_sibling_at_each_fork() {
        let s = mem();
        let conv = s.create_conversation("branched").unwrap();
        let base = 1_700_000_000_000i64;
        let mut user = Message::text(Role::User, "what".into());
        user.created_at = ts(base, 0);
        let mut a_old = Message::text(Role::Assistant, "old reply".into());
        a_old.created_at = ts(base, 1);
        let mut msgs = vec![user, a_old];
        s.save_messages(conv, &mut msgs).unwrap();
        let user_id = msgs[0].id.unwrap();
        let mut a_new = Message::text(Role::Assistant, "new reply".into());
        a_new.parent_id = Some(user_id);
        a_new.branch_index = 1;
        a_new.created_at = ts(base, 10);
        s.save_messages(conv, &mut [a_new]).unwrap();
        let loaded = s.load_messages(conv);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].text_str(), "what");
        assert_eq!(loaded[1].text_str(), "new reply");
        assert_eq!(loaded[1].branch_index, 1);
    }

    #[test]
    fn siblings_of_returns_all_branches_at_a_point() {
        let s = mem();
        let conv = s.create_conversation("sibs").unwrap();
        let mut msgs = vec![
            Message::text(Role::User, "ask".into()),
            Message::text(Role::Assistant, "one".into()),
        ];
        s.save_messages(conv, &mut msgs).unwrap();
        let user_id = msgs[0].id.unwrap();
        let first_a_id = msgs[1].id.unwrap();
        let mut sib = Message::text(Role::Assistant, "two".into());
        sib.parent_id = Some(user_id);
        sib.branch_index = 1;
        s.save_messages(conv, &mut [sib]).unwrap();
        let sibs = s.siblings_of(first_a_id);
        assert_eq!(sibs.len(), 2);
        assert_eq!(sibs[0].branch_index, 0);
        assert_eq!(sibs[1].branch_index, 1);
    }

    #[test]
    fn next_branch_index_grows_per_parent() {
        let s = mem();
        let conv = s.create_conversation("nbi").unwrap();
        let mut msgs = vec![Message::text(Role::User, "u".into())];
        s.save_messages(conv, &mut msgs).unwrap();
        let user_id = msgs[0].id.unwrap();
        assert_eq!(s.next_branch_index(conv, Some(user_id)), 0);
        let mut a = Message::text(Role::Assistant, "a".into());
        a.parent_id = Some(user_id);
        a.branch_index = 0;
        s.save_messages(conv, &mut [a]).unwrap();
        assert_eq!(s.next_branch_index(conv, Some(user_id)), 1);
    }

    #[test]
    fn round_trips_assistant_tool_calls_and_tool_result() {
        let s = mem();
        let conv = s.create_conversation("toolchat").unwrap();
        let user = Message::text(Role::User, "what's in foo.rs?".into());
        let mut assistant = Message::text(Role::Assistant, "I'll read it.".into());
        assistant.tool_calls = Some(vec![crate::message::ToolCall {
            id: "call_abc".into(),
            call_type: "function".into(),
            function: crate::message::ToolCallFunction {
                name: "read_file".into(),
                arguments: r#"{"path":"foo.rs"}"#.into(),
            },
        }]);
        let tool_result = Message::tool_result("call_abc".into(), "<file contents>".into());
        let mut msgs = vec![user, assistant, tool_result];
        s.save_messages(conv, &mut msgs).unwrap();
        let loaded = s.load_messages(conv);
        assert_eq!(loaded.len(), 3);
        let calls = loaded[1].tool_calls.as_ref().expect("tool_calls preserved");
        assert_eq!(calls[0].id, "call_abc");
        assert_eq!(calls[0].function.name, "read_file");
        assert_eq!(loaded[2].role, Role::Tool);
        assert_eq!(loaded[2].tool_call_id.as_deref(), Some("call_abc"));
        assert!(loaded[2].text_str().contains("<file contents>"));
    }

    #[test]
    fn working_dir_round_trips_in_settings() {
        let s = mem();
        let conv = s.create_conversation("wd").unwrap();
        let original = crate::config::ConversationSettings {
            working_dir: Some("/Users/heath/code/Fornax".into()),
            ..crate::config::ConversationSettings::default()
        };
        s.save_conversation_settings(conv, &original);
        let loaded = s.load_conversation_settings(conv);
        assert_eq!(
            loaded.working_dir.as_deref(),
            Some("/Users/heath/code/Fornax")
        );
    }

    #[test]
    fn walk_from_returns_subtree_active_path() {
        let s = mem();
        let conv = s.create_conversation("walk").unwrap();
        let base = 1_700_000_000_000i64;
        let mut u = Message::text(Role::User, "u".into());
        u.created_at = ts(base, 0);
        let mut a = Message::text(Role::Assistant, "a".into());
        a.created_at = ts(base, 1);
        let mut msgs = vec![u, a];
        s.save_messages(conv, &mut msgs).unwrap();
        let a_id = msgs[1].id.unwrap();

        // Branch A: user2_a -> assistant_a2
        let mut u2a = Message::text(Role::User, "u2a".into());
        u2a.parent_id = Some(a_id);
        u2a.created_at = ts(base, 10);
        let mut a2a = Message::text(Role::Assistant, "a2a".into());
        a2a.created_at = ts(base, 11);
        let mut branch_a = vec![u2a, a2a];
        s.save_messages(conv, &mut branch_a).unwrap();
        let u2a_id = branch_a[0].id;

        // Branch B: user2_b -> assistant_b2 (newer)
        let mut u2b = Message::text(Role::User, "u2b".into());
        u2b.parent_id = Some(a_id);
        u2b.branch_index = 1;
        u2b.created_at = ts(base, 20);
        let mut a2b = Message::text(Role::Assistant, "a2b".into());
        a2b.created_at = ts(base, 21);
        s.save_messages(conv, &mut [u2b, a2b]).unwrap();

        // walk_from(u2a) should return the A branch path
        let path = s.walk_from(u2a_id.unwrap());
        assert_eq!(path.len(), 2);
        assert_eq!(path[0].text_str(), "u2a");
        assert_eq!(path[1].text_str(), "a2a");
    }
}