//! Controlled filesystem command layer for MyPLC projects.
//! All project I/O goes through here so the frontend never touches raw FS APIs.
//! Every relative path is validated against the project root to block traversal.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

/// Tree listing limits: a project folder that accidentally contains something huge
/// (e.g. a copied node_modules) must not hang the IDE.
const MAX_TREE_DEPTH: usize = 32;
const MAX_TREE_ENTRIES: usize = 5_000;

/// Join `rel` onto `root`, rejecting anything that could escape the project root
/// (absolute paths, `..`, drive prefixes) or that names the root itself.
fn resolve_within(root: &str, rel: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    let mut named = false;
    for comp in rel_path.components() {
        match comp {
            Component::Normal(_) => named = true,
            Component::CurDir => {}
            _ => return Err("Invalid file path.".into()),
        }
    }
    if !named {
        return Err("Invalid file path.".into());
    }
    Ok(PathBuf::from(root).join(rel_path))
}

fn friendly(context: &str, err: std::io::Error) -> String {
    // Technical detail is logged; the caller shows the friendly text.
    eprintln!("[myplc] {context}: {err}");
    context.to_string()
}

const ALREADY_EXISTS: &str = "A file or folder with that name already exists.";

#[tauri::command]
pub fn create_project(
    root_path: String,
    project_json: String,
    main_content: String,
) -> Result<(), String> {
    let root = PathBuf::from(&root_path);
    if root.exists() {
        return Err("A folder with that name already exists at this location.".into());
    }
    for sub in ["programs", "functions", "function-blocks", ".myplc"] {
        fs::create_dir_all(root.join(sub))
            .map_err(|e| friendly("Unable to create the project folders.", e))?;
    }
    fs::write(root.join("project.json"), project_json)
        .map_err(|e| friendly("Unable to write project.json.", e))?;
    fs::write(root.join("programs").join("main.st"), main_content)
        .map_err(|e| friendly("Unable to create main.st.", e))?;
    Ok(())
}

#[tauri::command]
pub fn read_project(root_path: String) -> Result<String, String> {
    let path = Path::new(&root_path).join("project.json");
    if !path.exists() {
        return Err("Unable to open the selected project (project.json not found).".into());
    }
    fs::read_to_string(path).map_err(|e| friendly("Unable to open the selected project.", e))
}

#[tauri::command]
pub fn write_project_json(root_path: String, project_json: String) -> Result<(), String> {
    fs::write(Path::new(&root_path).join("project.json"), project_json)
        .map_err(|e| friendly("Unable to save the project.", e))
}

/// One file or folder in the project tree. `path` is relative to the project root and
/// always uses `/`, whatever the OS.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub children: Vec<TreeEntry>,
}

/// List the whole project: folders first, then files, each sorted by name
/// (case-insensitive). Symlinks are skipped so a link loop can't recurse forever.
#[tauri::command]
pub fn read_tree(root_path: String) -> Result<Vec<TreeEntry>, String> {
    let mut budget = MAX_TREE_ENTRIES;
    list_dir(Path::new(&root_path), "", 0, &mut budget).map_err(|e| friendly("Unable to read the project folder.", e))
}

fn list_dir(dir: &Path, rel: &str, depth: usize, budget: &mut usize) -> std::io::Result<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(dir)? {
        if *budget == 0 {
            break;
        }
        let item = item?;
        let file_type = item.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        // Names that aren't valid UTF-8 can't round-trip through the frontend.
        let Ok(name) = item.file_name().into_string() else { continue };
        *budget -= 1;
        let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let children = if file_type.is_dir() && depth < MAX_TREE_DEPTH {
            list_dir(&item.path(), &path, depth + 1, budget)?
        } else {
            Vec::new()
        };
        entries.push(TreeEntry { name, path, is_dir: file_type.is_dir(), children });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(entries)
}

#[tauri::command]
pub fn read_file(root_path: String, rel_path: String) -> Result<String, String> {
    let target = resolve_within(&root_path, &rel_path)?;
    fs::read_to_string(target).map_err(|e| friendly("Unable to open the file (it may not be a text file).", e))
}

/// Overwrite an existing file (saving an editor tab).
#[tauri::command]
pub fn write_file(root_path: String, rel_path: String, content: String) -> Result<(), String> {
    let target = resolve_within(&root_path, &rel_path)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| friendly("Unable to save the file.", e))?;
    }
    fs::write(target, content).map_err(|e| friendly("Unable to save the file.", e))
}

/// Create a new file; never overwrites an existing one.
#[tauri::command]
pub fn create_file(root_path: String, rel_path: String, content: String) -> Result<(), String> {
    let target = resolve_within(&root_path, &rel_path)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(target).map_err(|e| match e.kind() {
        ErrorKind::AlreadyExists => ALREADY_EXISTS.to_string(),
        _ => friendly("Unable to create the file.", e),
    })?;
    file.write_all(content.as_bytes()).map_err(|e| friendly("Unable to create the file.", e))
}

/// Create a new folder inside an existing one.
#[tauri::command]
pub fn create_dir(root_path: String, rel_path: String) -> Result<(), String> {
    let target = resolve_within(&root_path, &rel_path)?;
    fs::create_dir(target).map_err(|e| match e.kind() {
        ErrorKind::AlreadyExists => ALREADY_EXISTS.to_string(),
        _ => friendly("Unable to create the folder.", e),
    })
}

/// Rename (or move within the project) a file or folder.
#[tauri::command]
pub fn rename_file(root_path: String, old_rel_path: String, new_rel_path: String) -> Result<(), String> {
    let from = resolve_within(&root_path, &old_rel_path)?;
    let to = resolve_within(&root_path, &new_rel_path)?;
    // On case-insensitive filesystems `main.st` → `Main.st` "exists" already; it's the same
    // entry, so a case-only rename is allowed.
    let case_only = old_rel_path.to_lowercase() == new_rel_path.to_lowercase();
    if to.exists() && !case_only {
        return Err(ALREADY_EXISTS.into());
    }
    fs::rename(from, to).map_err(|e| friendly("Unable to rename.", e))
}

/// Delete a file, or a folder with everything in it.
#[tauri::command]
pub fn delete_entry(root_path: String, rel_path: String) -> Result<(), String> {
    let target = resolve_within(&root_path, &rel_path)?;
    // symlink_metadata: a link to a folder is removed as a link, never followed.
    let metadata = fs::symlink_metadata(&target).map_err(|e| friendly("Unable to delete: not found.", e))?;
    if metadata.is_dir() {
        fs::remove_dir_all(target).map_err(|e| friendly("Unable to delete the folder.", e))
    } else {
        fs::remove_file(target).map_err(|e| friendly("Unable to delete the file.", e))
    }
}

