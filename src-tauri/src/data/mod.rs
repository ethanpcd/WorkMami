//! 数据管理：按账号扫描 ~/.workbuddy 下的会话/记忆/技能/agent 设定/产出文件，
//! 支持浏览、查看、删除（入回收站）、导出、打包备份、跨账号迁移。

use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{WmError, WmResult};
use crate::platform::{workbuddy_paths, workmami_paths};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Sessions,
    Memory,
    AgentConfig,
    Skills,
    Artifacts,
}

impl Category {
    pub fn from_str(s: &str) -> WmResult<Category> {
        Ok(match s {
            "sessions" => Category::Sessions,
            "memory" => Category::Memory,
            "agent-config" => Category::AgentConfig,
            "skills" => Category::Skills,
            "artifacts" => Category::Artifacts,
            _ => return Err(WmError::Config(format!("未知类别: {s}"))),
        })
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Category::Sessions => "sessions",
            Category::Memory => "memory",
            Category::AgentConfig => "agent-config",
            Category::Skills => "skills",
            Category::Artifacts => "artifacts",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataItem {
    pub id: String,
    pub category: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub modified: i64,
    /// 会话附加信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// 列出某账号某类别的条目。uid 用于过滤账号级数据（None 表示全局）。
pub fn list(category: Category, uid: Option<&str>) -> WmResult<Vec<DataItem>> {
    let p = workbuddy_paths();
    let items = match category {
        Category::Sessions => list_sessions(&p.workbuddy_db, &p.sessions_dir, uid)?,
        Category::Memory => list_md_files(&[p.memory_dir.clone()], "memory")?,
        Category::AgentConfig => list_agent_config(uid)?,
        Category::Skills => list_skills(&p.skills_dir)?,
        Category::Artifacts => list_artifacts(&p.workspace)?,
    };
    // 会话/记忆可按 uid 前缀过滤（memory 文件名含 uid）
    let filtered = match (category, uid) {
        (Category::Memory, Some(u)) => items
            .into_iter()
            .filter(|i| i.name.contains(u) || i.path.contains(u))
            .collect(),
        _ => items,
    };
    Ok(filtered)
}

fn list_sessions(db: &Path, sessions_dir: &Path, uid: Option<&str>) -> WmResult<Vec<DataItem>> {
    let mut out = Vec::new();
    if db.exists() {
        // 先把 db 行读成 owned 元组，避免 stmt 借用 conn 与后续 out 变更的生命周期冲突
        let mut rows_data: Vec<(String, String, String, i64)> = Vec::new();
        {
            let conn = Connection::open_with_flags(
                db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )?;
            // 真实列：id / user_id / title / custom_title / model / created_at / updated_at / deleted_at。
            // custom_title 优先（用户改名），回落到 title；排除软删除；按 user_id 拆分账号。
            let (sql, args): (&str, Vec<String>) = match uid {
                Some(u) => (
                    "SELECT id,
                            COALESCE(NULLIF(custom_title, ''), title, ''),
                            COALESCE(model, ''),
                            COALESCE(updated_at, created_at, 0)
                     FROM sessions
                     WHERE deleted_at IS NULL AND user_id = ?1
                     ORDER BY 4 DESC
                     LIMIT 100000",
                    vec![u.to_string()],
                ),
                None => (
                    "SELECT id,
                            COALESCE(NULLIF(custom_title, ''), title, ''),
                            COALESCE(model, ''),
                            COALESCE(updated_at, created_at, 0)
                     FROM sessions
                     WHERE deleted_at IS NULL
                     ORDER BY 4 DESC
                     LIMIT 100000",
                    vec![],
                ),
            };
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| WmError::Config(format!("读取会话数据库失败: {e}")))?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                    let id: String = r.get(0)?;
                    let title: String = r.get(1).unwrap_or_default();
                    let model: String = r.get(2).unwrap_or_default();
                    let upd: i64 = r.get(3).unwrap_or(0);
                    Ok((id, title, model, upd))
                })
                .map_err(|e| WmError::Config(format!("读取会话失败: {e}")))?;
            for row in rows.flatten() {
                rows_data.push(row);
            }
        }
        for (id, title, model, upd) in rows_data {
            out.push(DataItem {
                id: id.clone(),
                category: "sessions".into(),
                name: if title.is_empty() { id.clone() } else { title },
                path: format!("db:sessions/{id}"),
                size: 0,
                modified: upd,
                meta: Some(serde_json::json!({ "model": model, "fromDb": true })),
            });
        }
    }
    // 文件型会话（sessions/*.json）：无法归属到具体账号，仅在全局视图（uid=None）展示，
    // 避免同一文件在多个账号下重复出现。
    if uid.is_none() && sessions_dir.exists() {
        for entry in walk(sessions_dir, 1)? {
            if entry.extension().and_then(|e| e.to_str()) == Some("json") {
                let md = std::fs::metadata(&entry).ok();
                out.push(DataItem {
                    id: file_key(&entry),
                    category: "sessions".into(),
                    name: entry.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string(),
                    path: entry.to_string_lossy().to_string(),
                    size: md.as_ref().map(|m| m.len()).unwrap_or(0),
                    modified: md
                        .as_ref()
                        .and_then(|m| m.modified().ok())
                        .map(|t| {
                            let d: chrono::DateTime<Utc> = t.into();
                            d.timestamp_millis()
                        })
                        .unwrap_or(0),
                    meta: Some(serde_json::json!({ "fromFile": true })),
                });
            }
        }
    }
    Ok(out)
}

fn list_md_files(dirs: &[PathBuf], cat: &str) -> WmResult<Vec<DataItem>> {
    let mut out = Vec::new();
    for d in dirs {
        if !d.exists() {
            continue;
        }
        for entry in walk(d, 2)? {
            if entry.extension().and_then(|e| e.to_str()) == Some("md") {
                let md = std::fs::metadata(&entry).ok();
                out.push(item_from_path(entry, cat, md));
            }
        }
    }
    Ok(out)
}

fn list_agent_config(uid: Option<&str>) -> WmResult<Vec<DataItem>> {
    let p = workbuddy_paths();
    let mut out = Vec::new();
    // 根级设定
    for name in ["IDENTITY.md", "SOUL.md", "USER.md", "MEMORY.md", "mcp.json", "models.json"] {
        let f = p.home.join(name);
        if f.exists() {
            let md = std::fs::metadata(&f).ok();
            out.push(item_from_path(f, "agent-config", md));
        }
    }
    if let Some(u) = uid {
        let personal = p.home.join(format!("user-{u}-personal"));
        if personal.exists() {
            for entry in walk(&personal, 1)? {
                if entry.is_file() {
                    let md = std::fs::metadata(&entry).ok();
                    out.push(item_from_path(entry, "agent-config", md));
                }
            }
        }
    }
    // rules 目录
    if p.rules_dir.exists() {
        for entry in walk(&p.rules_dir, 2)? {
            if entry.is_file() {
                let md = std::fs::metadata(&entry).ok();
                out.push(item_from_path(entry, "agent-config", md));
            }
        }
    }
    Ok(out)
}

fn list_skills(skills_dir: &Path) -> WmResult<Vec<DataItem>> {
    let mut out = Vec::new();
    if !skills_dir.exists() {
        return Ok(out);
    }
    if let Ok(rd) = std::fs::read_dir(skills_dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string();
                let size = dir_size(&path);
                out.push(DataItem {
                    id: name.clone(),
                    category: "skills".into(),
                    name,
                    path: path.to_string_lossy().to_string(),
                    size,
                    modified: 0,
                    meta: Some(serde_json::json!({ "dir": true })),
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn list_artifacts(workspace: &Path) -> WmResult<Vec<DataItem>> {
    let mut out = Vec::new();
    if !workspace.exists() {
        return Ok(out);
    }
    if let Ok(rd) = std::fs::read_dir(workspace) {
        for entry in rd.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let md = std::fs::metadata(&path).ok();
            let size = if path.is_dir() { dir_size(&path) } else { md.as_ref().map(|m| m.len()).unwrap_or(0) };
            out.push(DataItem {
                id: name.clone(),
                category: "artifacts".into(),
                name,
                path: path.to_string_lossy().to_string(),
                size,
                modified: md
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .map(|t| {
                        let d: chrono::DateTime<Utc> = t.into();
                        d.timestamp_millis()
                    })
                    .unwrap_or(0),
                meta: Some(serde_json::json!({ "isDir": path.is_dir() })),
            });
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(out)
}

/// 读取单个条目内容用于预览。db 会话解析 projects/ 下的 jsonl 为可读对话文本，文件返回文本。
pub fn read_item(item: &DataItem) -> WmResult<String> {
    if let Some(rest) = item.path.strip_prefix("db:sessions/") {
        // 真实对话内容在 ~/.workbuddy/projects/<项目>/<会话ID>.jsonl，db 只有元数据
        if let Some(text) = read_session_transcript(rest) {
            return Ok(text);
        }
        // 找不到 jsonl 时回落到 db 元数据行
        let p = workbuddy_paths();
        let conn = Connection::open_with_flags(
            &p.workbuddy_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        let mut stmt = conn.prepare("SELECT * FROM sessions WHERE id = ?1")?;
        let cols: Vec<String> = (0..stmt.column_count())
            .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
            .collect();
        let mut rows = stmt.query_map([rest], |row| {
            let mut map = serde_json::Map::new();
            for (i, c) in cols.iter().enumerate() {
                let v: rusqlite::types::Value = row.get(i)?;
                map.insert(c.clone(), value_to_json(v));
            }
            Ok(serde_json::Value::Object(map))
        })?;
        return match rows.next() {
            Some(Ok(v)) => Ok(serde_json::to_string_pretty(&v)?),
            _ => Err(WmError::NotFound("会话不存在".into())),
        };
    }
    let path = Path::new(&item.path);
    if path.is_dir() {
        // 目录：返回文件清单
        let files = walk(path, 3)?;
        let list: Vec<String> = files
            .iter()
            .map(|f| f.to_string_lossy().to_string())
            .collect();
        return Ok(list.join("\n"));
    }
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

/// 删除：移动到 WorkMami 回收站（带时间戳），保留相对路径。
pub fn delete(items: &[DataItem]) -> WmResult<usize> {
    let trash = workmami_paths().trash_dir.join(Utc::now().format("%Y%m%d-%H%M%S").to_string());
    std::fs::create_dir_all(&trash)?;
    let mut n = 0;
    for it in items {
        if let Some(rest) = it.path.strip_prefix("db:sessions/") {
            // db 会话不物理删（风险高），仅归档导出
            let _ = rest;
            continue;
        }
        let src = Path::new(&it.path);
        if !src.exists() {
            continue;
        }
        let rel = relative_to_home(src);
        let dst = trash.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        move_path(src, &dst)?;
        n += 1;
    }
    Ok(n)
}

// ---------- 会话正文（jsonl）解析 ----------

/// 在 ~/.workbuddy/projects/**/{sid}.jsonl 中定位会话正文文件。
fn find_session_jsonl(sid: &str) -> Option<PathBuf> {
    let projects = workbuddy_paths().home.join("projects");
    if !projects.exists() {
        return None;
    }
    let Ok(rd) = std::fs::read_dir(&projects) else { return None };
    for entry in rd.flatten() {
        let cand = entry.path().join(format!("{sid}.jsonl"));
        if cand.is_file() {
            return Some(cand);
        }
    }
    // 兜底：项目可能再嵌套一层，浅层遍历一次
    if let Ok(all) = walk(&projects, 3) {
        return all.into_iter().find(|f| {
            f.file_name().and_then(|s| s.to_str()) == Some(&format!("{sid}.jsonl"))
        });
    }
    None
}

/// 把 jsonl 中的 message 行渲染成可读对话文本；无正文或解析失败返回 None。
fn read_session_transcript(sid: &str) -> Option<String> {
    let path = find_session_jsonl(sid)?;
    let bytes = std::fs::read(&path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut out = String::new();
    out.push_str(&format!("会话正文（来源: {}）\n", path.to_string_lossy()));
    out.push_str(&"=".repeat(60));
    out.push('\n');
    let mut count = 0u32;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue;
        }
        let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("?");
        let label = match role {
            "user" => "【用户】",
            "assistant" => "【助手】",
            other => other,
        };
        let mut msg = String::new();
        match v.get("content") {
            Some(serde_json::Value::String(s)) => msg.push_str(s),
            Some(serde_json::Value::Array(parts)) => {
                for p in parts {
                    let pt = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    let t = p.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    match pt {
                        "input_text" | "output_text" | "text" => {
                            if !t.is_empty() {
                                if !msg.is_empty() {
                                    msg.push('\n');
                                }
                                msg.push_str(t);
                            }
                        }
                        _ => {} // reasoning / function_call 等不混入正文
                    }
                }
            }
            _ => {}
        }
        if msg.trim().is_empty() {
            continue;
        }
        // 用户消息里注入的系统上下文做裁剪，只留真人输入
        if role == "user" {
            msg = strip_user_context(&msg);
            if msg.trim().is_empty() {
                continue;
            }
        }
        let ts = v.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
        out.push_str(&format!("\n{label}  {}\n{}\n", fmt_ts(ts), msg.trim()));
        count += 1;
    }
    if count == 0 {
        return None;
    }
    out.push_str(&format!("\n{}\n共 {count} 条对话消息\n", "=".repeat(60)));
    Some(out)
}

/// 用户消息常带 <system-reminder data-role="user-context">…</system-reminder> 注入头，
/// 尽量裁掉，只保留真实输入；裁剪后为空则回落原文截断。
fn strip_user_context(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("<system-reminder") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</system-reminder>") {
            Some(rel) => rest = &rest[start + rel + "</system-reminder>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    let trimmed = out.trim().to_string();
    if trimmed.is_empty() {
        s.chars().take(2000).collect()
    } else {
        trimmed
    }
}

fn fmt_ts(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

/// 导出单条目到目标目录，返回落盘路径。
pub fn export(item: &DataItem, dest_dir: &Path) -> WmResult<PathBuf> {
    std::fs::create_dir_all(dest_dir)?;
    let content = read_item(item)?;
    let out = dest_dir.join(sanitize(&item.name) + ".txt");
    std::fs::write(&out, content)?;
    Ok(out)
}

/// 打包备份：把给定条目打包成 zip。dest_dir 指定时写入该目录，否则写入 WorkMami/Backups，返回 zip 路径。
pub fn backup(items: &[DataItem], label: &str, dest_dir: Option<&str>) -> WmResult<PathBuf> {
    let backups = match dest_dir {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => workmami_paths().backups_dir.clone(),
    };
    std::fs::create_dir_all(&backups)?;
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let zip_path = backups.join(format!("workmami-backup-{label}-{stamp}.zip"));
    let file = std::fs::File::create(&zip_path)?;
    let mut writer = zip::ZipWriter::new(std::io::BufWriter::new(file));

    let mut manifest = Vec::new();
    for it in items {
        if let Some(rest) = it.path.strip_prefix("db:sessions/") {
            let name = format!("sessions/{rest}.txt");
            // 优先备份可读对话正文，找不到才回落 db 元数据
            let content = read_session_transcript(rest)
                .or_else(|| read_item(it).ok())
                .unwrap_or_else(|| "{}".into());
            writer.start_file(&name, deflate_opts())?;
            use std::io::Write;
            writer.write_all(content.as_bytes())?;
            manifest.push(serde_json::json!({ "id": it.id, "category": it.category, "in": name }));
            continue;
        }
        let src = Path::new(&it.path);
        if src.is_dir() {
            for f in walk(src, 6)? {
                if !f.is_file() {
                    continue;
                }
                let rel = f.strip_prefix(src).unwrap_or(&f);
                let name = format!("{}/{}/{}", it.category, sanitize(&it.name), rel.to_string_lossy());
                if add_file_to_zip(&mut writer, &name, &f).is_ok() {
                    manifest.push(serde_json::json!({ "id": it.id, "category": it.category, "in": name }));
                }
            }
        } else if src.is_file() {
            let name = format!("{}/{}", it.category, sanitize(&it.name));
            if add_file_to_zip(&mut writer, &name, src).is_ok() {
                manifest.push(serde_json::json!({ "id": it.id, "category": it.category, "in": name }));
            }
        }
    }
    // manifest.json
    let mf = serde_json::json!({
        "tool": "WorkMami",
        "version": env!("CARGO_PKG_VERSION"),
        "label": label,
        "createdAt": Utc::now().to_rfc3339(),
        "items": manifest,
    });
    writer.start_file("manifest.json", deflate_opts())?;
    use std::io::Write;
    writer.write_all(serde_json::to_vec_pretty(&mf)?.as_slice())?;
    writer.finish()?;
    Ok(zip_path)
}

/// 构造 deflate 压缩选项（不显式标注类型，交由推导，兼容多版本 zip）。
fn deflate_opts() -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

fn add_file_to_zip<W: std::io::Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    name: &str,
    path: &Path,
) -> WmResult<()> {
    writer.start_file(name, deflate_opts())?;
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut buf)?;
    use std::io::Write;
    writer.write_all(&buf)?;
    Ok(())
}

/// 跨账号迁移：把 from_uid 的 memory/agent 设定/技能复制到 to_uid 对应目录。
/// 冲突时重命名保留双方。返回迁移条目数。
pub fn migrate(from_uid: &str, to_uid: &str, categories: &[Category]) -> WmResult<usize> {
    let p = workbuddy_paths();
    let mut n = 0;
    for cat in categories {
        match cat {
            Category::Memory => {
                let src = p.memory_dir.join(format!("{from_uid}_memory.md"));
                if src.exists() {
                    let dst = p.memory_dir.join(format!("{to_uid}_memory.md"));
                    copy_preserve(&src, &dst)?;
                    n += 1;
                }
            }
            Category::AgentConfig => {
                let src = p.home.join(format!("user-{from_uid}-personal"));
                if src.exists() {
                    let dst = p.home.join(format!("user-{to_uid}-personal"));
                    copy_tree_preserve(&src, &dst)?;
                    n += 1;
                }
            }
            Category::Skills => {
                // 技能是全局目录，迁移语义为“确保存在”，这里跳过实际复制
                let _ = p.skills_dir;
            }
            Category::Sessions => {
                n += migrate_sessions(&p.workbuddy_db, from_uid, to_uid)?;
            }
            _ => {}
        }
    }
    Ok(n)
}

/// 迁移会话：把 from_uid 的全部会话行的 user_id 改到 to_uid。
/// 写入前先整库备份（含 -wal/-shm）；设置 busy_timeout，若被 WorkBuddy 占用则报错提示退出后重试。
/// 会话正文 jsonl 按项目全局存储、不带 uid，预览/导出按 sid 全局查找，故无需搬移正文。
fn migrate_sessions(db: &Path, from_uid: &str, to_uid: &str) -> WmResult<usize> {
    if !db.exists() {
        return Ok(0);
    }
    // 1) 整库备份，便于回滚
    backup_db_file(db)?;
    // 2) 读写连接 + 忙等待超时（被其它进程写锁占用时最多等 3s 再报错）
    let mut conn = Connection::open(db)?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    // 3) 统计待迁移条数
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE user_id = ?1",
            rusqlite::params![from_uid],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if count == 0 {
        return Ok(0);
    }
    // 4) 事务内改归属
    let tx = conn.transaction().map_err(|e| {
        WmError::Config(format!("迁移会话失败（若 WorkBuddy 正在运行，请退出后重试）: {e}"))
    })?;
    tx.execute(
        "UPDATE sessions SET user_id = ?1 WHERE user_id = ?2",
        rusqlite::params![to_uid, from_uid],
    )
    .map_err(|e| {
        WmError::Config(format!("迁移会话失败（若 WorkBuddy 正在运行，请退出后重试）: {e}"))
    })?;
    tx.commit()
        .map_err(|e| WmError::Config(format!("提交会话迁移失败: {e}")))?;
    Ok(count as usize)
}

/// 把 workbuddy.db（及 -wal/-shm）整库复制到 WorkMami 备份目录，带时间戳，便于回滚。
fn backup_db_file(db: &Path) -> WmResult<()> {
    let backups = workmami_paths().backups_dir.clone();
    std::fs::create_dir_all(&backups)?;
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let name = db
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workbuddy.db");
    let dst = backups.join(format!("{name}.pre-migrate-{stamp}"));
    std::fs::copy(db, &dst)?;
    for ext in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{ext}", db.to_string_lossy()));
        if side.exists() {
            let _ = std::fs::copy(&side, backups.join(format!("{name}.pre-migrate-{stamp}{ext}")));
        }
    }
    Ok(())
}

// ---------- 工具函数 ----------

fn item_from_path(path: PathBuf, cat: &str, md: Option<std::fs::Metadata>) -> DataItem {
    DataItem {
        id: file_key(&path),
        category: cat.into(),
        name: path.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string(),
        path: path.to_string_lossy().to_string(),
        size: md.as_ref().map(|m| m.len()).unwrap_or(0),
        modified: md
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                let d: chrono::DateTime<Utc> = t.into();
                d.timestamp_millis()
            })
            .unwrap_or(0),
        meta: None,
    }
}

fn file_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn walk(dir: &Path, depth: u32) -> WmResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let mut stack = vec![(dir.to_path_buf(), 0u32)];
    while let Some((d, dep)) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if dep < depth {
                        stack.push((p, dep + 1));
                    }
                } else {
                    out.push(p);
                }
            }
        }
    }
    Ok(out)
}

fn dir_size(dir: &Path) -> u64 {
    walk(dir, 6)
        .unwrap_or_default()
        .iter()
        .filter_map(|f| std::fs::metadata(f).ok())
        .map(|m| m.len())
        .sum()
}

fn relative_to_home(path: &Path) -> PathBuf {
    let home = workbuddy_paths().home.clone();
    path.strip_prefix(&home)
        .unwrap_or(path)
        .to_path_buf()
}

fn move_path(src: &Path, dst: &Path) -> WmResult<()> {
    if std::fs::rename(src, dst).is_err() {
        // 跨设备：复制后删除
        if src.is_dir() {
            copy_tree(src, dst)?;
            std::fs::remove_dir_all(src)?;
        } else {
            std::fs::copy(src, dst)?;
            std::fs::remove_file(src)?;
        }
    }
    Ok(())
}

fn copy_tree(src: &Path, dst: &Path) -> WmResult<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let p = e.path();
        let d = dst.join(e.file_name());
        if p.is_dir() {
            copy_tree(&p, &d)?;
        } else {
            std::fs::copy(&p, &d)?;
        }
    }
    Ok(())
}

fn copy_preserve(src: &Path, dst: &Path) -> WmResult<()> {
    if dst.exists() {
        let stamped = dst.with_extension(format!("migrated.{}.md", Utc::now().timestamp()));
        std::fs::copy(src, stamped)?;
        Ok(())
    } else {
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::copy(src, dst)?;
        Ok(())
    }
}

fn copy_tree_preserve(src: &Path, dst: &Path) -> WmResult<()> {
    if !dst.exists() {
        return copy_tree(src, dst);
    }
    // 逐文件复制，冲突重命名
    for f in walk(src, 6)? {
        let rel = f.strip_prefix(src).unwrap_or(&f);
        let target = dst.join(rel);
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p)?;
        }
        if target.exists() {
            let stamped = target.with_extension(format!("migrated.{}", Utc::now().timestamp()));
            std::fs::copy(&f, stamped)?;
        } else {
            std::fs::copy(&f, &target)?;
        }
    }
    Ok(())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

fn value_to_json(v: rusqlite::types::Value) -> serde_json::Value {
    use rusqlite::types::Value as RV;
    match v {
        RV::Null => serde_json::Value::Null,
        RV::Integer(i) => serde_json::json!(i),
        RV::Real(f) => serde_json::json!(f),
        RV::Text(s) => serde_json::Value::String(s),
        RV::Blob(b) => serde_json::Value::String(format!("<blob {} bytes>", b.len())),
    }
}
