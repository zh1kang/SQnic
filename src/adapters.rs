//! Project-local lifecycle adapter installation.
//!
//! This module only edits adapter-owned configuration.  It does not edit
//! instruction files, user-global configuration, or Codex's hook trust state.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::Harness;

const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "Stop",
    "SessionEnd",
];
const PI_TEMPLATE: &str = include_str!("../adapters/pi-extension.ts");

/// Install or remove the project-local adapter for one harness.
///
/// The operation is idempotent.  Existing non-SQnic entries are retained, and
/// malformed or ambiguous configuration is rejected before any write occurs.
pub fn install(repo: &str, harness: Harness, db: &Path, remove: bool) -> Result<Value> {
    anyhow::ensure!(
        !cfg!(windows),
        "automatic adapter setup currently requires a Unix host; manual CLI import remains available on Windows"
    );
    let repo = canonical_repo(repo)?;
    let db = absolute_path(db)?;
    let executable = fs::canonicalize(std::env::current_exe()?)
        .context("resolve the running sqnic executable")?;

    match harness.as_str() {
        "claude" => install_json_adapter(
            &repo,
            harness,
            &db,
            &executable,
            &repo.join(".claude/settings.json"),
            remove,
        ),
        "codex" => install_json_adapter(
            &repo,
            harness,
            &db,
            &executable,
            &repo.join(".codex/hooks.json"),
            remove,
        ),
        "pi" => install_pi(&repo, harness, &db, &executable, remove),
        "cursor" => install_cursor(&repo, harness, &db, &executable, remove),
        other => bail!("unsupported harness `{other}`"),
    }
}

fn install_cursor(
    repo: &Path,
    harness: Harness,
    db: &Path,
    executable: &Path,
    remove: bool,
) -> Result<Value> {
    let path = repo.join(".cursor/hooks.json");
    if remove && !path.exists() {
        return Ok(json!({"installed":false,"changed":false,"harness":"cursor"}));
    }
    ensure_parent_directory(path.parent().context("config parent")?)?;
    let JsonConfig {
        object: mut config,
        original,
    } = read_json_object(&path)?;
    if let Some(version) = config.get("version") {
        anyhow::ensure!(
            version == &json!(1),
            "unsupported Cursor hook configuration version"
        );
    }
    let prefix = format!(": {}; ", shell_quote(Path::new(&marker(repo, harness))));
    let command = format!("{prefix}{}", hook_command(executable, repo, harness, db));
    let expected = json!({"command":command,"timeout":3});
    let events = [
        "sessionStart",
        "sessionEnd",
        "beforeSubmitPrompt",
        "postToolUse",
        "postToolUseFailure",
        "afterAgentResponse",
        "stop",
    ];
    let hooks = config
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("Cursor hooks must be an object")?;
    let mut changed = false;
    for event in events {
        if remove && !hooks.contains_key(event) {
            continue;
        }
        let entries = hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("Cursor event hooks must be an array")?;
        let owned: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e["command"]
                    .as_str()
                    .is_some_and(|c| c.starts_with(&prefix))
            })
            .map(|(i, _)| i)
            .collect();
        anyhow::ensure!(owned.len() <= 1, "duplicate SQnic Cursor hooks");
        if let Some(&index) = owned.first() {
            anyhow::ensure!(
                entries[index] == expected,
                "SQnic Cursor hook was modified; repair it before setup"
            );
            if remove {
                entries.remove(index);
                changed = true;
            }
        } else if !remove {
            entries.push(expected.clone());
            changed = true;
        }
        if entries.is_empty() {
            hooks.remove(event);
        }
    }
    if !remove {
        config.insert("version".into(), json!(1));
    }
    if changed {
        write_json_atomic(&path, &config, original.as_deref())?;
    }
    Ok(
        json!({"harness":"cursor","path":path,"installed":!remove,"changed":changed,"coverage":"lifecycle observations; transcript import remains explicit","notice":"startup context delivery depends on Cursor version; use restore if context is absent"}),
    )
}

fn canonical_repo(input: &str) -> Result<PathBuf> {
    let path = Path::new(input);
    let repo = fs::canonicalize(path)
        .with_context(|| format!("canonicalize repository path `{}`", path.display()))?;
    if !repo.is_dir() {
        bail!("repository path is not a directory: {}", repo.display());
    }
    Ok(repo)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn install_json_adapter(
    repo: &Path,
    harness: Harness,
    db: &Path,
    executable: &Path,
    config_path: &Path,
    remove: bool,
) -> Result<Value> {
    let parent = config_path
        .parent()
        .context("adapter config has no parent")?;
    if remove && !parent.exists() {
        return Ok(
            json!({"harness": harness.as_str(), "path": config_path, "installed": false, "changed": false}),
        );
    }
    ensure_parent_directory(parent)?;
    let JsonConfig {
        object: mut root,
        original,
    } = read_json_object(config_path)?;
    let command = hook_command(executable, repo, harness, db);
    let marker = marker(repo, harness);
    let mut changed = false;

    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = hooks
        .as_object_mut()
        .context("adapter config `hooks` must be a JSON object")?;

    for event in EVENTS {
        let expected = managed_entry(&command, &marker, event);
        let Some(value) = hooks.get_mut(*event) else {
            if !remove {
                hooks.insert((*event).to_string(), Value::Array(vec![expected]));
                changed = true;
            }
            continue;
        };
        let entries = value
            .as_array_mut()
            .with_context(|| format!("adapter config hook `{event}` must be an array"))?;
        let mut found = false;
        let mut retained = Vec::with_capacity(entries.len());
        for entry in entries.drain(..) {
            let own_marker = entry_has_marker(&entry, &marker);
            let own_command = entry_contains_command(&entry, &command);
            if (own_marker || own_command) && entry != expected {
                bail!(
                    "SQnic-owned or conflicting `{event}` hook has been changed or grouped; remove it manually or restore it before retrying"
                );
            }
            if own_marker || own_command {
                found = true;
                if !remove {
                    retained.push(expected.clone());
                } else {
                    changed = true;
                }
            } else {
                retained.push(entry);
            }
        }
        if !found && !remove {
            retained.push(expected);
            changed = true;
        }
        *entries = retained;
    }

    if remove {
        let empty_hooks = if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut)
        {
            hooks.retain(|_, value| {
                value
                    .as_array()
                    .map(|items| !items.is_empty())
                    .unwrap_or(true)
            });
            hooks.is_empty()
        } else {
            false
        };
        if empty_hooks {
            root.remove("hooks");
        }
    }

    if changed {
        write_json_atomic(config_path, &root, original.as_deref())?;
    }
    Ok(json!({
        "harness": harness.as_str(),
        "path": config_path,
        "installed": !remove,
        "changed": changed,
        "events": EVENTS,
    }))
}

fn install_pi(
    repo: &Path,
    harness: Harness,
    db: &Path,
    executable: &Path,
    remove: bool,
) -> Result<Value> {
    let directory = repo.join(".pi/extensions");
    if remove {
        if !check_directory_chain(repo, &directory, false)? {
            return Ok(
                json!({"harness": harness.as_str(), "path": directory.join("sqnic.ts"), "installed": false, "changed": false}),
            );
        }
    } else {
        check_directory_chain(repo, &directory, true)?;
    }
    let path = directory.join("sqnic.ts");
    let content = PI_TEMPLATE
        .replace(
            "__SQNIC_BIN__",
            &typescript_string(&executable.display().to_string()),
        )
        .replace(
            "__SQNIC_DB__",
            &typescript_string(&db.display().to_string()),
        )
        .replace(
            "__SQNIC_REPO__",
            &typescript_string(&repo.display().to_string()),
        );
    let existing = read_optional_bytes(&path)?;
    let marker = "SQNIC_ADAPTER_V1";
    let own = existing
        .as_deref()
        .map(|bytes| String::from_utf8_lossy(bytes).contains(marker))
        .unwrap_or(false);
    if remove {
        if own {
            if existing.as_deref() != Some(content.as_bytes()) {
                bail!(
                    "refusing to remove modified SQnic adapter `{}`",
                    path.display()
                );
            }
            remove_owned_file(&path)?;
            return Ok(
                json!({"harness": harness.as_str(), "path": path, "installed": false, "changed": true}),
            );
        }
        if existing.is_some() {
            bail!(
                "refusing to remove unrelated Pi extension `{}`",
                path.display()
            );
        }
        return Ok(
            json!({"harness": harness.as_str(), "path": path, "installed": false, "changed": false}),
        );
    }
    if let Some(bytes) = existing.as_deref() {
        if !own {
            bail!(
                "Pi adapter path already exists and is not SQnic-owned: {}",
                path.display()
            );
        }
        if bytes == content.as_bytes() {
            return Ok(
                json!({"harness": harness.as_str(), "path": path, "installed": true, "changed": false}),
            );
        }
        bail!("SQnic Pi adapter was modified; restore it before retrying");
    }
    write_bytes_atomic(&path, content.as_bytes(), existing.as_deref())?;
    Ok(json!({"harness": harness.as_str(), "path": path, "installed": true, "changed": true}))
}

fn managed_entry(command: &str, marker: &str, event: &str) -> Value {
    let mut hook = Map::new();
    hook.insert("type".into(), Value::String("command".into()));
    hook.insert("command".into(), Value::String(command.into()));
    hook.insert("statusMessage".into(), Value::String(marker.into()));
    if event == "SessionStart" || event == "UserPromptSubmit" {
        hook.insert(
            "additionalContextLimit".into(),
            Value::Number(serde_json::Number::from(8000)),
        );
    } else {
        hook.insert("timeout".into(), Value::Number(serde_json::Number::from(3)));
    }
    json!({"hooks": [Value::Object(hook)]})
}

fn entry_contains_command(entry: &Value, expected: &str) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| {
            hooks
                .iter()
                .any(|hook| hook.get("command").and_then(Value::as_str) == Some(expected))
        })
        .unwrap_or(false)
}

fn entry_has_marker(entry: &Value, expected: &str) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| {
            hooks
                .iter()
                .any(|hook| hook.get("statusMessage").and_then(Value::as_str) == Some(expected))
        })
        .unwrap_or(false)
}

fn marker(repo: &Path, harness: Harness) -> String {
    format!("sqnic-managed:v1:{}:{}", harness.as_str(), repo.display())
}

fn hook_command(executable: &Path, repo: &Path, harness: Harness, db: &Path) -> String {
    format!(
        "{} hook --repo {} --harness {} --db {}",
        shell_quote(executable),
        shell_quote(repo),
        shell_quote(Path::new(harness.as_str())),
        shell_quote(db),
    )
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn typescript_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings are always JSON serializable")
}

struct JsonConfig {
    object: Map<String, Value>,
    original: Option<Vec<u8>>,
}
fn read_json_object(path: &Path) -> Result<JsonConfig> {
    let Some(bytes) = read_optional_bytes(path)? else {
        return Ok(JsonConfig {
            object: Map::new(),
            original: None,
        });
    };
    let value: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse adapter config `{}`", path.display()))?;
    let object = value.as_object().cloned().with_context(|| {
        format!(
            "adapter config `{}` must contain a JSON object",
            path.display()
        )
    })?;
    Ok(JsonConfig {
        object,
        original: Some(bytes),
    })
}

fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                bail!("refusing to edit symlink `{}`", path.display());
            }
            Ok(Some(
                fs::read(path).with_context(|| format!("read `{}`", path.display()))?,
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspect `{}`", path.display())),
    }
}

fn ensure_parent_directory(path: &Path) -> Result<()> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!(
                "refusing to use unsafe adapter directory `{}`",
                path.display()
            );
        }
    } else {
        fs::create_dir_all(path).with_context(|| format!("create `{}`", path.display()))?;
    }
    Ok(())
}

fn check_directory_chain(root: &Path, target: &Path, create: bool) -> Result<bool> {
    let relative = target.strip_prefix(root).with_context(|| {
        format!(
            "adapter directory is outside repository: {}",
            target.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    bail!(
                        "refusing to use unsafe adapter directory `{}`",
                        current.display()
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                fs::create_dir(&current)
                    .with_context(|| format!("create `{}`", current.display()))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(error).with_context(|| format!("inspect `{}`", current.display()));
            }
        }
    }
    Ok(true)
}

fn write_json_atomic(
    path: &Path,
    value: &Map<String, Value>,
    original: Option<&[u8]>,
) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    write_bytes_atomic(path, &bytes, original)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8], original: Option<&[u8]>) -> Result<()> {
    let parent = path.parent().context("adapter file has no parent")?;
    ensure_parent_directory(parent)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = parent.join(format!(".sqnic-{}.{}.tmp", std::process::id(), stamp));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("create temporary adapter file `{}`", temporary.display()))?;
    if let Ok(metadata) = fs::metadata(path) {
        file.set_permissions(metadata.permissions())?;
    } else {
        set_private_permissions(&file)?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        let _ = fs::remove_file(&temporary);
        bail!("refusing to replace symlink `{}`", path.display());
    }
    if read_optional_bytes(path)?.as_deref() != original {
        fs::remove_file(&temporary)?;
        bail!("adapter configuration changed during setup; retry");
    }
    fs::rename(&temporary, path).with_context(|| format!("replace `{}`", path.display()))?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn remove_owned_file(path: &Path) -> Result<()> {
    fs::remove_file(path).with_context(|| format!("remove `{}`", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(file: &File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_file: &File) -> Result<()> {
    Ok(())
}
