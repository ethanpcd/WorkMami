//! 用量库（workmami-usage.db）：记录反代请求，并提供看板聚合查询。
//! 同时把 WorkBuddy 本地 session_usage 表作为 “app” 来源并入，覆盖 App 内消耗。

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::WmResult;
use crate::platform::workmami_paths;

/// 单次请求记录。
#[derive(Debug, Clone, Serialize)]
pub struct RequestLog {
    pub ts: i64, // unix ms
    pub uid: String,
    pub model: String,
    pub session_hash: String,
    pub source: String, // proxy / app
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub reasoning_tokens: i64,
    pub cached_tokens: i64,
    pub total_tokens: i64,
    pub credits: f64,
    pub latency_ms: i64,
    pub ttft_ms: i64,
    pub status: i64,
    pub ok: bool,
    pub ua: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub total_tokens: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub reasoning_tokens: i64,
    pub cached_tokens: i64,
    pub credits: f64,
    pub requests: i64,
    pub success: i64,
    pub success_rate: f64,
    pub avg_tokens_per_req: f64,
    pub cache_hit_rate: f64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendPoint {
    pub bucket: String,
    pub tokens: i64,
    pub credits: f64,
    pub requests: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopItem {
    pub key: String,
    pub label: String,
    pub tokens: i64,
    pub credits: f64,
    pub requests: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeatCell {
    pub weekday: i64, // 0=Mon
    pub hour: i64,    // 0-23
    pub tokens: i64,
}

/// 单条请求流水（看板明细表用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRow {
    pub ts: i64,
    pub uid: String,
    pub model: String,
    pub source: String, // proxy / app
    pub total_tokens: i64,
    pub credits: f64,
    pub latency_ms: i64,
    pub status: i64,
    pub ok: bool,
}

pub struct UsageStore {
    conn: Mutex<Connection>,
}

impl UsageStore {
    pub fn open() -> WmResult<Self> {
        let path = workmami_paths().usage_db.clone();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS requests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts INTEGER NOT NULL,
                uid TEXT NOT NULL,
                model TEXT NOT NULL,
                session_hash TEXT NOT NULL,
                source TEXT NOT NULL,
                prompt_tokens INTEGER NOT NULL,
                completion_tokens INTEGER NOT NULL,
                reasoning_tokens INTEGER NOT NULL,
                cached_tokens INTEGER NOT NULL,
                total_tokens INTEGER NOT NULL,
                credits REAL NOT NULL,
                latency_ms INTEGER NOT NULL,
                ttft_ms INTEGER NOT NULL,
                status INTEGER NOT NULL,
                ok INTEGER NOT NULL,
                ua TEXT NOT NULL,
                dedup TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_req_ts ON requests(ts);
            CREATE INDEX IF NOT EXISTS idx_req_model ON requests(model);
            CREATE INDEX IF NOT EXISTS idx_req_uid ON requests(uid);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_req_dedup ON requests(dedup) WHERE dedup IS NOT NULL;
            "#,
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn record(&self, r: &RequestLog) -> WmResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO requests(ts,uid,model,session_hash,source,prompt_tokens,completion_tokens,
                reasoning_tokens,cached_tokens,total_tokens,credits,latency_ms,ttft_ms,status,ok,ua,dedup)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,NULL)"#,
            params![
                r.ts, r.uid, r.model, r.session_hash, r.source,
                r.prompt_tokens, r.completion_tokens, r.reasoning_tokens, r.cached_tokens,
                r.total_tokens, r.credits, r.latency_ms, r.ttft_ms, r.status,
                r.ok as i64, r.ua
            ],
        )?;
        Ok(())
    }

    /// 从 workbuddy.db 导入 session_usage 作为 app 来源（按 session_id 去重）。
    pub fn import_app_usage(&self, wb_db: &std::path::Path, uid: &str) -> WmResult<u64> {
        if !wb_db.exists() {
            return Ok(0);
        }
        let src = Connection::open_with_flags(
            wb_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        // sessions 表提供 model / 标题；session_usage 提供 used/size/credit_json
        let mut stmt = src.prepare(
            "SELECT su.session_id, su.used, su.size, su.credit_json, su.updated_at,
                    COALESCE(s.model,'') 
             FROM session_usage su LEFT JOIN sessions s ON s.id = su.session_id",
        )?;
        let rows = stmt.query_map([], |row| {
            let sid: String = row.get(0)?;
            let used: i64 = row.get(1)?;
            let _size: i64 = row.get(2)?;
            let credit: Option<String> = row.get(3)?;
            let updated: i64 = row.get(4)?;
            let model: String = row.get(5).unwrap_or_default();
            Ok((sid, used, credit, updated, model))
        })?;
        let mut n = 0u64;
        {
            let conn = self.conn.lock().unwrap();
            for row in rows.flatten() {
                let (sid, used, credit, updated, model) = row;
                // credit_json 真实结构是 {请求ID: 该次消耗积分} 的映射，
                // 会话总消耗 = 所有 value 求和。兼容旧的 {"credit":x}/{"credits":x} 单值写法。
                let credits: f64 = credit
                    .as_deref()
                    .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
                    .map(|v| match &v {
                        serde_json::Value::Object(map) => {
                            if let Some(x) = map.get("credit").or(map.get("credits")).and_then(|x| x.as_f64()) {
                                x
                            } else {
                                map.values().filter_map(|x| x.as_f64()).sum()
                            }
                        }
                        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
                        _ => 0.0,
                    })
                    .unwrap_or(0.0);
                let dedup = format!("app:{sid}");
                let r = conn.execute(
                    r#"INSERT OR IGNORE INTO requests(ts,uid,model,session_hash,source,prompt_tokens,
                        completion_tokens,reasoning_tokens,cached_tokens,total_tokens,credits,latency_ms,
                        ttft_ms,status,ok,ua,dedup)
                        VALUES(?1,?2,?3,?4,'app',?5,0,0,0,?5,?6,0,0,200,1,'workbuddy-app',?7)"#,
                    params![updated, uid, model, sid, used, credits, dedup],
                )?;
                n += r as u64;
            }
        }
        Ok(n)
    }

    fn query_range<T>(
        &self,
        sql: &str,
        from: i64,
        to: i64,
        map: impl Fn(&rusqlite::Row) -> rusqlite::Result<T>,
    ) -> WmResult<Vec<T>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        let out = stmt
            .query_map(params![from, to], map)?
            .flatten()
            .collect();
        Ok(out)
    }

    pub fn summary(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> WmResult<Summary> {
        let (f, t) = bounds(from, to);
        let conn = self.conn.lock().unwrap();
        let s = conn.query_row(
            r#"SELECT
                 COALESCE(SUM(total_tokens),0),
                 COALESCE(SUM(prompt_tokens),0),
                 COALESCE(SUM(completion_tokens),0),
                 COALESCE(SUM(reasoning_tokens),0),
                 COALESCE(SUM(cached_tokens),0),
                 COALESCE(SUM(credits),0),
                 COUNT(*),
                 COALESCE(SUM(ok),0),
                 COALESCE(AVG(total_tokens),0),
                 COALESCE(AVG(latency_ms),0)
               FROM requests WHERE ts BETWEEN ?1 AND ?2"#,
            params![f, t],
            |r| {
                Ok(Summary {
                    total_tokens: r.get(0)?,
                    prompt_tokens: r.get(1)?,
                    completion_tokens: r.get(2)?,
                    reasoning_tokens: r.get(3)?,
                    cached_tokens: r.get(4)?,
                    credits: r.get(5)?,
                    requests: r.get(6)?,
                    success: r.get(7)?,
                    avg_tokens_per_req: r.get(8)?,
                    avg_latency_ms: r.get(9)?,
                    success_rate: 0.0,
                    cache_hit_rate: 0.0,
                })
            },
        )?;
        let mut s = s;
        s.success_rate = if s.requests > 0 { s.success as f64 / s.requests as f64 * 100.0 } else { 0.0 };
        s.cache_hit_rate = if s.prompt_tokens > 0 {
            s.cached_tokens as f64 / s.prompt_tokens as f64 * 100.0
        } else {
            0.0
        };
        Ok(s)
    }

    /// 趋势：granularity ∈ day/week/month。
    pub fn trend(&self, from: DateTime<Utc>, to: DateTime<Utc>, granularity: &str) -> WmResult<Vec<TrendPoint>> {
        let (f, t) = bounds(from, to);
        let fmt = match granularity {
            "month" => "%Y-%m",
            "week" => "%Y-W%W",
            _ => "%Y-%m-%d",
        };
        let sql = format!(
            r#"SELECT strftime('{fmt}', ts/1000, 'unixepoch', 'localtime') AS bucket,
                      COALESCE(SUM(total_tokens),0), COALESCE(SUM(credits),0), COUNT(*)
               FROM requests WHERE ts BETWEEN ?1 AND ?2
               GROUP BY bucket ORDER BY bucket"#
        );
        self.query_range(
            &sql,
            f,
            t,
            |r| {
                Ok(TrendPoint {
                    bucket: r.get(0)?,
                    tokens: r.get(1)?,
                    credits: r.get(2)?,
                    requests: r.get(3)?,
                })
            },
        )
    }

    pub fn top_models(&self, from: DateTime<Utc>, to: DateTime<Utc>, limit: i64) -> WmResult<Vec<TopItem>> {
        let (f, t) = bounds(from, to);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"SELECT model, COALESCE(SUM(total_tokens),0), COALESCE(SUM(credits),0), COUNT(*)
               FROM requests WHERE ts BETWEEN ?1 AND ?2 AND model<>''
               GROUP BY model ORDER BY SUM(total_tokens) DESC LIMIT ?3"#,
        )?;
        let out = stmt
            .query_map(params![f, t, limit], |r| {
                Ok(TopItem {
                    key: r.get(0)?,
                    label: r.get::<_, String>(0)?,
                    tokens: r.get(1)?,
                    credits: r.get(2)?,
                    requests: r.get(3)?,
                })
            })?
            .flatten()
            .collect();
        Ok(out)
    }

    pub fn top_sessions(&self, from: DateTime<Utc>, to: DateTime<Utc>, limit: i64) -> WmResult<Vec<TopItem>> {
        let (f, t) = bounds(from, to);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"SELECT session_hash, COALESCE(SUM(total_tokens),0), COALESCE(SUM(credits),0), COUNT(*)
               FROM requests WHERE ts BETWEEN ?1 AND ?2 AND session_hash<>''
               GROUP BY session_hash ORDER BY SUM(total_tokens) DESC LIMIT ?3"#,
        )?;
        let out = stmt
            .query_map(params![f, t, limit], |r| {
                let h: String = r.get(0)?;
                Ok(TopItem {
                    label: short_hash(&h),
                    key: h,
                    tokens: r.get(1)?,
                    credits: r.get(2)?,
                    requests: r.get(3)?,
                })
            })?
            .flatten()
            .collect();
        Ok(out)
    }

    pub fn per_account(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> WmResult<Vec<TopItem>> {
        let (f, t) = bounds(from, to);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"SELECT uid, COALESCE(SUM(total_tokens),0), COALESCE(SUM(credits),0), COUNT(*)
               FROM requests WHERE ts BETWEEN ?1 AND ?2
               GROUP BY uid ORDER BY SUM(credits) DESC"#,
        )?;
        let out = stmt
            .query_map(params![f, t], |r| {
                let u: String = r.get(0)?;
                Ok(TopItem {
                    label: format!("账号 …{}", tail(&u, 6)),
                    key: u,
                    tokens: r.get(1)?,
                    credits: r.get(2)?,
                    requests: r.get(3)?,
                })
            })?
            .flatten()
            .collect();
        Ok(out)
    }

    /// 最近请求流水（时间倒序），用于看板"请求走哪个账号"明细表。
    pub fn recent_requests(&self, from: DateTime<Utc>, to: DateTime<Utc>, limit: i64) -> WmResult<Vec<RequestRow>> {
        let (f, t) = bounds(from, to);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"SELECT ts, uid, model, source, total_tokens, credits, latency_ms, status, ok
               FROM requests WHERE ts BETWEEN ?1 AND ?2
               ORDER BY ts DESC LIMIT ?3"#,
        )?;
        let out = stmt
            .query_map(params![f, t, limit], |r| {
                Ok(RequestRow {
                    ts: r.get(0)?,
                    uid: r.get(1)?,
                    model: r.get(2)?,
                    source: r.get(3)?,
                    total_tokens: r.get(4)?,
                    credits: r.get(5)?,
                    latency_ms: r.get(6)?,
                    status: r.get(7)?,
                    ok: r.get::<_, i64>(8)? != 0,
                })
            })?
            .flatten()
            .collect();
        Ok(out)
    }

    pub fn heatmap(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> WmResult<Vec<HeatCell>> {
        let (f, t) = bounds(from, to);
        let sql = r#"SELECT
                        ((strftime('%w', ts/1000,'unixepoch','localtime')+6)%7) AS weekday,
                        CAST(strftime('%H', ts/1000,'unixepoch','localtime') AS INTEGER) AS hour,
                        COALESCE(SUM(total_tokens),0)
                     FROM requests WHERE ts BETWEEN ?1 AND ?2
                     GROUP BY weekday, hour"#;
        self.query_range(sql, f, t, |r| {
            Ok(HeatCell {
                weekday: r.get(0)?,
                hour: r.get(1)?,
                tokens: r.get(2)?,
            })
        })
    }

    /// 记录一条积分余额快照（用于余额阶梯线，复用 requests 之外可另建表；这里存 settings 侧）。
    pub fn count_all(&self) -> WmResult<i64> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))?;
        Ok(n)
    }
}

fn bounds(from: DateTime<Utc>, to: DateTime<Utc>) -> (i64, i64) {
    (from.timestamp_millis(), to.timestamp_millis())
}

fn short_hash(h: &str) -> String {
    if h.len() <= 12 {
        h.to_string()
    } else {
        format!("{}…", &h[..12])
    }
}

fn tail(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        s[s.len() - n..].to_string()
    }
}
