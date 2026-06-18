use rusqlite::{Connection, params};

/// Aggregated usage for the Usage page. Built from the `usage` table — one row
/// per completed stream turn (tool-loop turns included).
#[derive(Default)]
pub struct UsageStats {
    pub total_requests: i64,
    pub ok_requests: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
    /// Latency/throughput percentiles across all recorded turns.
    pub ttft_p50_ms: Option<f64>,
    pub ttft_p95_ms: Option<f64>,
    pub decode_p50_tok_s: Option<f64>,
    pub decode_p95_tok_s: Option<f64>,
    pub by_model: Vec<UsageByModel>,
    pub daily: Vec<UsageDaily>,
}

pub struct UsageByModel {
    pub model: String,
    pub endpoint: String,
    pub requests: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub cost: f64,
    pub avg_ttft_ms: Option<f64>,
    pub avg_decode_tok_s: Option<f64>,
}

pub struct UsageDaily {
    pub date: String,
    pub total_tokens: i64,
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

pub struct UsageStorage {
    conn: Connection,
}

impl Default for UsageStorage {
    fn default() -> Self {
        let conn = Connection::open(Self::db_path()).expect("Failed to open database");
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .ok();
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        Self { conn }
    }
}

impl UsageStorage {
    fn db_path() -> std::path::PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("fornax")
            .join("fornax.db")
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

    /// Aggregate the `usage` table for the Usage page: grand totals, a per-model
    /// breakdown (newest-first by request count), and a daily token series.
    pub fn usage_stats(&self) -> UsageStats {
        let mut stats = UsageStats::default();

        if let Ok(row) = self.conn.query_row(
            "SELECT COUNT(*), \
                    COALESCE(SUM(ok), 0), \
                    COALESCE(SUM(prompt_tokens), 0), \
                    COALESCE(SUM(completion_tokens), 0), \
                    COALESCE(SUM(cost), 0) \
             FROM usage",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, f64>(4)?,
                ))
            },
        ) {
            stats.total_requests = row.0;
            stats.ok_requests = row.1;
            stats.prompt_tokens = row.2;
            stats.completion_tokens = row.3;
            stats.total_tokens = row.2 + row.3;
            stats.total_cost = row.4;
        }

        stats.ttft_p50_ms = self.usage_percentile("ttft_ms", 0.50);
        stats.ttft_p95_ms = self.usage_percentile("ttft_ms", 0.95);
        stats.decode_p50_tok_s = self.usage_percentile("decode_tok_s", 0.50);
        stats.decode_p95_tok_s = self.usage_percentile("decode_tok_s", 0.95);

        if let Ok(mut stmt) = self.conn.prepare(
            "SELECT model, endpoint, COUNT(*), \
                    COALESCE(SUM(prompt_tokens), 0), \
                    COALESCE(SUM(completion_tokens), 0), \
                    COALESCE(SUM(cost), 0), \
                    AVG(ttft_ms), AVG(decode_tok_s) \
             FROM usage \
             GROUP BY model, endpoint \
             ORDER BY COUNT(*) DESC",
        ) {
            if let Ok(rows) = stmt.query_map([], |r| {
                let prompt: i64 = r.get(3)?;
                let completion: i64 = r.get(4)?;
                Ok(UsageByModel {
                    model: r.get(0)?,
                    endpoint: r.get(1)?,
                    requests: r.get(2)?,
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                    total_tokens: prompt + completion,
                    cost: r.get(5)?,
                    avg_ttft_ms: r.get(6)?,
                    avg_decode_tok_s: r.get(7)?,
                })
            }) {
                stats.by_model = rows.filter_map(Result::ok).collect();
            }
        }

        if let Ok(mut stmt) = self.conn.prepare(
            "SELECT date(ts), COALESCE(SUM(prompt_tokens + completion_tokens), 0) \
             FROM usage \
             GROUP BY date(ts) \
             ORDER BY date(ts)",
        ) {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok(UsageDaily {
                    date: r.get(0)?,
                    total_tokens: r.get(1)?,
                })
            }) {
                stats.daily = rows.filter_map(Result::ok).collect();
            }
        }

        stats
    }

    /// Wipe all recorded usage (the Usage page's "clear" action).
    pub fn clear_usage(&self) {
        self.conn.execute("DELETE FROM usage", []).ok();
    }

    /// Enforce the usage retention window: delete rows older than `days`.
    /// `0` is a no-op (keep forever). Returns the number of rows removed.
    pub fn prune_usage(&self, days: u32) -> usize {
        if days == 0 {
            return 0;
        }
        let cutoff = format!("-{days} days");
        self.conn
            .execute(
                "DELETE FROM usage WHERE ts < datetime('now', ?1)",
                params![cutoff],
            )
            .unwrap_or(0)
    }

    /// Nearest-rank percentile of a non-null usage column (`q` in 0.0..=1.0).
    /// Returns `None` when there are no samples. `col` is a compile-time
    /// literal, never user input.
    fn usage_percentile(&self, col: &str, q: f64) -> Option<f64> {
        let sql = format!(
            "SELECT {col} FROM usage WHERE {col} IS NOT NULL ORDER BY {col}"
        );
        let mut stmt = self.conn.prepare(&sql).ok()?;
        let values: Vec<f64> = stmt
            .query_map([], |r| r.get::<_, f64>(0))
            .ok()?
            .filter_map(Result::ok)
            .collect();
        if values.is_empty() {
            return None;
        }
        let rank = (q * values.len() as f64).ceil() as usize;
        let idx = rank.saturating_sub(1).min(values.len() - 1);
        Some(values[idx])
    }

    #[cfg(test)]
    pub(crate) fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("open :memory:");
        conn.execute_batch("PRAGMA foreign_keys=ON;").ok();
        conn.execute_batch(
            "CREATE TABLE usage (
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
        .expect("create usage table");
        Self { conn }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> UsageStorage {
        UsageStorage::new_in_memory()
    }

    #[test]
    fn usage_record_and_aggregate() {
        let s = mem();
        assert_eq!(s.usage_stats().total_requests, 0);
        let rec = |model: &'static str, p: u32, c: u32| UsageRecord {
            endpoint: "http://localhost:42069/v1",
            model,
            prompt_tokens: Some(p),
            completion_tokens: Some(c),
            ttft_ms: Some(100.0),
            decode_tok_s: Some(50.0),
            cost: Some(0.01),
            ok: true,
        };
        s.record_usage(&rec("mlx-a", 10, 20));
        s.record_usage(&rec("mlx-a", 5, 15));
        s.record_usage(&rec("mlx-b", 100, 200));
        let stats = s.usage_stats();
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.ok_requests, 3);
        assert_eq!(stats.prompt_tokens, 115);
        assert_eq!(stats.completion_tokens, 235);
        assert_eq!(stats.total_tokens, 350);
        assert!((stats.total_cost - 0.03).abs() < 1e-9);
        assert_eq!(stats.ttft_p50_ms, Some(100.0));
        assert_eq!(stats.ttft_p95_ms, Some(100.0));
        assert_eq!(stats.decode_p50_tok_s, Some(50.0));
        assert_eq!(stats.by_model.len(), 2);
        assert_eq!(stats.by_model[0].model, "mlx-a");
        assert_eq!(stats.by_model[0].requests, 2);
        assert_eq!(stats.by_model[0].total_tokens, 50);
        assert!((stats.by_model[0].cost - 0.02).abs() < 1e-9);
        assert_eq!(stats.by_model[1].model, "mlx-b");
        assert_eq!(stats.daily.len(), 1);
        assert_eq!(stats.daily[0].total_tokens, 350);
        s.clear_usage();
        assert_eq!(s.usage_stats().total_requests, 0);
    }

    #[test]
    fn usage_prune_respects_retention() {
        let s = mem();
        let rec = UsageRecord {
            endpoint: "ep",
            model: "m",
            prompt_tokens: Some(1),
            completion_tokens: Some(1),
            ttft_ms: None,
            decode_tok_s: None,
            cost: None,
            ok: true,
        };
        s.record_usage(&rec);
        s.conn
            .execute(
                "INSERT INTO usage (ts, endpoint, model, prompt_tokens, completion_tokens, ok) \
                 VALUES (datetime('now', '-100 days'), 'ep', 'm', 1, 1, 1)",
                [],
            )
            .unwrap();
        assert_eq!(s.usage_stats().total_requests, 2);
        assert_eq!(s.prune_usage(0), 0);
        assert_eq!(s.usage_stats().total_requests, 2);
        assert_eq!(s.prune_usage(30), 1);
        assert_eq!(s.usage_stats().total_requests, 1);
    }
}