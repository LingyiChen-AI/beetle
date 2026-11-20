//! 技能中心详情弹窗：列出技能目录的文件树、读取单个文本文件（只读）。
//!
//! 安全：技能名必须是技能中心里存在的技能；文件路径必须是相对路径，任何一段是 `..`、带盘符或根目录的一律拒绝，
//! 规范化（canonicalize，解开符号链接）后还必须落在技能目录内。只读文本：超过 512KB 或是二进制的只返回提示。

use crate::catalog::{catalog_dir, list_manifests};
use crate::commands::blocking;
use serde::Serialize;
use std::path::{Component, Path, PathBuf};

/// 单个文件最多读多少字节。
pub const MAX_READ_BYTES: u64 = 512 * 1024;
/// 文件树最多列多少项（防止误把大目录打包进来时卡界面）。
const MAX_ENTRIES: usize = 2000;
/// 最深几层。
const MAX_DEPTH: u32 = 8;
/// 这些目录不展示。
const SKIP_DIRS: [&str; 4] = ["__pycache__", ".git", "node_modules", ".venv"];
/// 判断二进制时看开头多少字节。
const SNIFF_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileEntry {
    /// 相对技能目录，正斜杠分隔
    pub path: String,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    /// 0 = 技能目录下第一层
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileContent {
    pub path: String,
    pub size: u64,
    /// 文本内容；二进制或超过上限时为 None
    pub content: Option<String>,
    pub binary: bool,
    pub too_large: bool,
}

/// 技能中心里这个技能的目录；不存在的技能报错。
fn skill_root(name: &str) -> Result<PathBuf, String> {
    if !list_manifests().iter().any(|m| m.name == name) {
        return Err(format!("技能中心里没有技能 {name}"));
    }
    Ok(catalog_dir().join(name))
}

fn sort_key(e: &std::fs::DirEntry) -> (bool, String) {
    let is_file = !e.file_type().map(|t| t.is_dir()).unwrap_or(false);
    (is_file, e.file_name().to_string_lossy().to_lowercase())
}

fn walk(root: &Path, dir: &Path, depth: u32, out: &mut Vec<FileEntry>) -> Result<(), String> {
    if depth > MAX_DEPTH || out.len() >= MAX_ENTRIES {
        return Ok(());
    }
    let rd = std::fs::read_dir(dir).map_err(|e| format!("读取目录 {} 失败：{e}", dir.display()))?;
    let mut entries: Vec<std::fs::DirEntry> = rd.filter_map(|e| e.ok()).collect();
    // 目录在前、文件在后，同类按名字（不分大小写）
    entries.sort_by_key(sort_key);
    for e in entries {
        if out.len() >= MAX_ENTRIES {
            break;
        }
        let Ok(ft) = e.file_type() else { continue };
        // 不跟随符号链接，避免列出技能目录以外的东西
        if ft.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if ft.is_dir() && SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let path = e.path();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let size = if ft.is_dir() { 0 } else { e.metadata().map(|m| m.len()).unwrap_or(0) };
        out.push(FileEntry { path: rel, name, size, is_dir: ft.is_dir(), depth });
        if ft.is_dir() {
            walk(root, &path, depth + 1, out)?;
        }
    }
    Ok(())
}

/// 列出 `root` 下的文件树（先序：目录紧跟着它的子项）。
pub fn list_files(root: &Path) -> Result<Vec<FileEntry>, String> {
    let mut out = Vec::new();
    walk(root, root, 0, &mut out)?;
    Ok(out)
}

/// 把前端传来的相对路径解析成 `root` 下的真实文件路径；路径穿越、绝对路径、目录一律拒绝。
pub fn resolve_in(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim();
    if rel.is_empty() || rel.contains('\0') {
        return Err("文件路径为空或不合法".into());
    }
    // 正反斜杠都当分隔符检查，任何一段是 .. 就拒绝（Windows 和 Unix 上同样严格）
    if rel.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(format!("文件路径不能包含 ..：{rel}"));
    }
    let p = Path::new(rel);
    if p.is_absolute() || p.has_root() || p.components().any(|c| matches!(c, Component::Prefix(_) | Component::RootDir | Component::ParentDir)) {
        return Err(format!("只能读取技能目录里的相对路径：{rel}"));
    }
    // 自己按正反斜杠切再拼，不靠 `Path` 的平台语义：Windows 端传来的 `scripts\run.py`
    // 在 Unix 上会被当成一个含反斜杠的文件名，怎么都读不到。
    let normalized: PathBuf = rel.split(['/', '\\']).filter(|s| !s.is_empty() && *s != ".").collect();
    let base = root.canonicalize().map_err(|e| format!("技能目录不可用：{e}"))?;
    let full = base.join(&normalized).canonicalize().map_err(|_| format!("文件不存在：{rel}"))?;
    if !full.starts_with(&base) {
        return Err(format!("只能读取技能目录里的文件：{rel}"));
    }
    if !full.is_file() {
        return Err(format!("不是文件：{rel}"));
    }
    Ok(full)
}

/// 读 `root` 下的一个文本文件。
pub fn read_file_in(root: &Path, rel: &str) -> Result<FileContent, String> {
    let full = resolve_in(root, rel)?;
    let size = std::fs::metadata(&full).map_err(|e| format!("读取文件信息失败：{e}"))?.len();
    let path = rel.trim().replace('\\', "/");
    if size > MAX_READ_BYTES {
        return Ok(FileContent { path, size, content: None, binary: false, too_large: true });
    }
    let bytes = std::fs::read(&full).map_err(|e| format!("读取文件失败：{e}"))?;
    let head = &bytes[..bytes.len().min(SNIFF_BYTES)];
    if head.contains(&0) {
        return Ok(FileContent { path, size, content: None, binary: true, too_large: false });
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(FileContent { path, size, content: Some(text.trim_start_matches('\u{feff}').to_string()), binary: false, too_large: false }),
        Err(_) => Ok(FileContent { path, size, content: None, binary: true, too_large: false }),
    }
}

/// 技能目录的文件树。
#[tauri::command]
pub async fn catalog_files(name: String) -> Result<Vec<FileEntry>, String> {
    blocking(move || list_files(&skill_root(&name)?)).await
}

/// 读技能目录里的一个文本文件（相对路径，限 512KB）。
#[tauri::command]
pub async fn catalog_read_file(name: String, path: String) -> Result<FileContent, String> {
    blocking(move || read_file_in(&skill_root(&name)?, &path)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时目录：skill/ 是技能目录，outside.txt 在它外面。
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skill");
        std::fs::create_dir_all(root.join("scripts").join("__pycache__")).unwrap();
        std::fs::create_dir_all(root.join("_shared")).unwrap();
        std::fs::write(root.join("SKILL.md"), "\u{feff}# 查 bug\n说明").unwrap();
        std::fs::write(root.join("catalog.json"), "{\"name\":\"x\"}").unwrap();
        std::fs::write(root.join("scripts").join("run.py"), "print('hi')\n").unwrap();
        std::fs::write(root.join("scripts").join("__pycache__").join("run.pyc"), [0u8, 1, 2]).unwrap();
        std::fs::write(root.join("_shared").join("common.py"), "X = 1\n").unwrap();
        std::fs::write(root.join("logo.bin"), [0x89u8, b'P', 0, 0, 1]).unwrap();
        std::fs::write(root.join("big.txt"), "a".repeat(MAX_READ_BYTES as usize + 1)).unwrap();
        std::fs::write(root.join("latin1.txt"), [0xE9u8, 0x74, 0xE9]).unwrap();
        std::fs::write(dir.path().join("outside.txt"), "secret").unwrap();
        (dir, root)
    }

    #[test]
    fn lists_tree_dirs_first_with_depth_and_skips_cache() {
        let (_d, root) = fixture();
        let files = list_files(&root).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["_shared", "_shared/common.py", "scripts", "scripts/run.py", "big.txt", "catalog.json", "latin1.txt", "logo.bin", "SKILL.md"]
        );
        let run = files.iter().find(|f| f.path == "scripts/run.py").unwrap();
        assert_eq!((run.name.as_str(), run.depth, run.is_dir, run.size), ("run.py", 1, false, 12));
        let scripts = files.iter().find(|f| f.path == "scripts").unwrap();
        assert!(scripts.is_dir && scripts.depth == 0 && scripts.size == 0);
        assert!(!paths.iter().any(|p| p.contains("__pycache__")));
    }

    #[test]
    fn reads_text_and_strips_bom() {
        let (_d, root) = fixture();
        let f = read_file_in(&root, "SKILL.md").unwrap();
        assert_eq!(f.content.as_deref(), Some("# 查 bug\n说明"));
        assert!(!f.binary && !f.too_large);
        let f = read_file_in(&root, "scripts/run.py").unwrap();
        assert_eq!(f.content.as_deref(), Some("print('hi')\n"));
        assert_eq!(f.path, "scripts/run.py");
        // 反斜杠写法也认，返回时统一成正斜杠
        let f = read_file_in(&root, "scripts\\run.py").unwrap();
        assert_eq!(f.path, "scripts/run.py");
    }

    #[test]
    fn binary_and_too_large_return_hints_not_content() {
        let (_d, root) = fixture();
        let b = read_file_in(&root, "logo.bin").unwrap();
        assert!(b.binary && b.content.is_none());
        let l = read_file_in(&root, "latin1.txt").unwrap();
        assert!(l.binary && l.content.is_none(), "不是 UTF-8 的当二进制处理");
        let big = read_file_in(&root, "big.txt").unwrap();
        assert!(big.too_large && big.content.is_none());
        assert_eq!(big.size, MAX_READ_BYTES + 1);
    }

    #[test]
    fn path_traversal_and_absolute_paths_are_rejected() {
        let (d, root) = fixture();
        for bad in ["../outside.txt", "scripts/../../outside.txt", "scripts\\..\\..\\outside.txt", "./../outside.txt", "..", "", "   "] {
            assert!(read_file_in(&root, bad).is_err(), "应拒绝 {bad:?}");
        }
        let abs = d.path().join("outside.txt");
        assert!(read_file_in(&root, &abs.to_string_lossy()).is_err(), "绝对路径应拒绝");
        assert!(read_file_in(&root, "/outside.txt").is_err());
        #[cfg(windows)]
        {
            assert!(read_file_in(&root, "C:outside.txt").is_err());
            assert!(read_file_in(&root, "\\\\?\\C:\\Windows\\win.ini").is_err());
        }
        // 目录、不存在的文件也拒绝
        assert!(read_file_in(&root, "scripts").unwrap_err().contains("不是文件"));
        assert!(read_file_in(&root, "nope.md").unwrap_err().contains("文件不存在"));
    }

    #[test]
    fn real_catalog_skill_is_listed_and_guarded() {
        let files = list_files(&skill_root("bug-hunt").unwrap()).unwrap();
        assert!(files.iter().any(|f| f.path == "SKILL.md" && !f.is_dir));
        assert!(skill_root("not-a-skill").unwrap_err().contains("没有技能"));
        assert!(skill_root("../catalog").is_err());
        let root = skill_root("bug-hunt").unwrap();
        assert!(read_file_in(&root, "SKILL.md").unwrap().content.is_some());
        assert!(read_file_in(&root, "../bug-hunt/SKILL.md").is_err());
    }
}
