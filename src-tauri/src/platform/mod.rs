//! 平台抽象层：集中解析 WorkBuddy 与 WorkMami 自身的所有落盘路径。
//!
//! macOS 与 Windows 上 WorkBuddy 的目录布局不同，这里通过编译期 cfg 与
//! 运行期探测统一暴露给上层，避免路径字符串散落各处。

use std::path::PathBuf;

use once_cell::sync::OnceCell;

/// 当前平台标识，前端展示用。
pub fn platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

/// WorkMami 相关路径集合。
#[derive(Debug, Clone)]
pub struct WorkmamiPaths {
    /// ~/Library/Application Support/WorkMami 或 %APPDATA%\WorkMami
    pub root: PathBuf,
    pub accounts_file: PathBuf,
    pub settings_file: PathBuf,
    pub usage_db: PathBuf,
    pub backups_dir: PathBuf,
    pub trash_dir: PathBuf,
    pub logs_dir: PathBuf,
}

/// WorkBuddy 官方客户端相关路径集合（只读扫描 / 谨慎写入）。
#[derive(Debug, Clone)]
pub struct WorkbuddyPaths {
    /// ~/.workbuddy （macOS=家目录，Windows=%USERPROFILE%）
    pub home: PathBuf,
    /// ~/.workbuddy/security —— 按 uid 分目录，用于发现历史数据账号
    pub security_dir: PathBuf,
    /// 依次探测的 workbuddy-desktop.info 候选绝对路径。
    /// 覆盖 macOS(~/Library/Application Support) 与 Windows(%APPDATA% / %LOCALAPPDATA%)、
    /// 多种应用目录名（CodeBuddyExtension / WorkBuddy / CodeBuddy）与子路径。
    pub desktop_info_candidates: Vec<PathBuf>,
    /// ~/.workbuddy/keyblob
    pub keyblob: PathBuf,
    /// ~/.workbuddy-key-fallback
    pub key_fallback: PathBuf,
    /// ~/.workbuddy/workbuddy.db
    pub workbuddy_db: PathBuf,
    pub sessions_dir: PathBuf,
    pub memory_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub rules_dir: PathBuf,
    /// ~/WorkBuddy （产出工作区）
    pub workspace: PathBuf,
}

static WM_PATHS: OnceCell<WorkmamiPaths> = OnceCell::new();
static WB_PATHS: OnceCell<WorkbuddyPaths> = OnceCell::new();

fn app_support_base() -> PathBuf {
    // macOS: ~/Library/Application Support  Windows: %APPDATA%  Linux: ~/.local/share
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// 初始化并缓存全部路径。应用启动时调用一次。
pub fn init_paths() {
    let _ = workmami_paths();
    let _ = workbuddy_paths();
}

pub fn workmami_paths() -> &'static WorkmamiPaths {
    WM_PATHS.get_or_init(|| {
        let root = app_support_base().join("WorkMami");
        WorkmamiPaths {
            accounts_file: root.join("accounts.json"),
            settings_file: root.join("settings.json"),
            usage_db: root.join("workmami-usage.db"),
            backups_dir: root.join("Backups"),
            trash_dir: root.join("trash"),
            logs_dir: root.join("logs"),
            root,
        }
    })
}

pub fn workbuddy_paths() -> &'static WorkbuddyPaths {
    WB_PATHS.get_or_init(|| {
        let home = home_dir();
        let wb_home = home.join(".workbuddy");

        // desktop.info 候选：多基础目录 × 多应用目录名 × 多子路径 × 多文件名。
        // macOS 上三者通常同为 ~/Library/Application Support，去重即可。
        let mut bases: Vec<PathBuf> = Vec::new();
        for b in [
            dirs::data_dir(),
            dirs::config_dir(),
            dirs::data_local_dir(),
        ] {
            if let Some(b) = b {
                if !bases.iter().any(|x| x == &b) {
                    bases.push(b);
                }
            }
        }
        let app_names = ["CodeBuddyExtension", "WorkBuddy", "CodeBuddy"];
        let subpaths: [&[&str]; 2] = [
            &["Data", "Public", "auth"],
            &["auth"],
        ];
        let file_names = ["workbuddy-desktop.info", "workbuddy-desktop-dev.info"];
        let mut candidates: Vec<PathBuf> = Vec::new();
        for base in &bases {
            for app in app_names {
                for sub in subpaths {
                    for f in file_names {
                        let mut p = base.join(app);
                        for seg in sub {
                            p = p.join(seg);
                        }
                        let p = p.join(f);
                        if !candidates.iter().any(|x| x == &p) {
                            candidates.push(p);
                        }
                    }
                }
            }
        }

        WorkbuddyPaths {
            home: wb_home.clone(),
            security_dir: wb_home.join("security"),
            desktop_info_candidates: candidates,
            keyblob: wb_home.join("keyblob"),
            key_fallback: home.join(".workbuddy-key-fallback"),
            workbuddy_db: wb_home.join("workbuddy.db"),
            sessions_dir: wb_home.join("sessions"),
            memory_dir: wb_home.join("memory"),
            skills_dir: wb_home.join("skills"),
            rules_dir: wb_home.join("rules"),
            workspace: home.join("WorkBuddy"),
        }
    })
}

impl WorkbuddyPaths {
    /// 返回实际存在的 desktop.info 文件（按候选顺序探测）。
    pub fn existing_info_file(&self) -> Option<PathBuf> {
        self.desktop_info_candidates
            .iter()
            .find(|p| p.exists())
            .cloned()
    }
}
