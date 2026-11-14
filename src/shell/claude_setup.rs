//! Connects TaskbarHWM to Claude Code through its documented `statusLine` setting.
//!
//! Claude Code runs the configured command on each status line update and passes session data (including the 5-hour
//! and weekly usage limits) as JSON on stdin. The command we configure is a tiny PowerShell script that saves a few
//! of those fields for the widget. PowerShell ships with Windows and runs unelevated, so Claude Code launching it
//! never triggers an administrator prompt (launching this app's own exe would, since it runs elevated).
//!
//! `settings.json` is edited as text instead of with a JSON library, to keep the app small. Only the one
//! `statusLine` entry is added or removed; everything else in the file is left exactly as it was.
use std::path::Path;

use crate::sensors::claude::{claude_dir, data_dir, Link};

const SCRIPT: &str = include_str!("../../res/claude-relay.ps1");
const SCRIPT_NAME: &str = "claude-relay.ps1";
const KEY: &str = "\"statusLine\"";

/// The command Claude Code runs. Forward slashes and double quotes work in both Git Bash and PowerShell, the two
/// shells Claude Code uses on Windows for status line commands.
fn relay_command(script: &Path) -> String {
    let p = script.to_string_lossy().replace('\\', "/");
    format!("powershell -NoProfile -ExecutionPolicy Bypass -File \"{p}\"")
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn has_status_line(text: &str) -> bool {
    text.contains(KEY)
}

/// `text` (a settings.json) with a `statusLine` entry for `command` added as its first key.
fn insert_status_line(text: &str, command: &str) -> Result<String, String> {
    let t = text.trim_start_matches('\u{feff}');
    let open = t.find('{').filter(|&i| t[..i].trim().is_empty()).ok_or("Claude Code's settings.json doesn't look like a JSON object.")?;
    if has_status_line(t) {
        return Err("Claude Code already has a status line.".into());
    }
    let rest = &t[open + 1..];
    let empty = rest.trim_start().starts_with('}');
    let entry = format!("\n  {KEY}: {{ \"type\": \"command\", \"command\": \"{}\" }}", json_escape(command));
    Ok(format!("{}{}{}{}", &t[..=open], entry, if empty { "" } else { "," }, rest))
}

/// Index just past the `}` that closes the object starting at `open` (a `{`), skipping braces inside strings.
fn object_end(t: &str, open: usize) -> Option<usize> {
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, c) in t[open..].char_indices() {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, '\\') => esc = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// `text` without its `statusLine` entry, but only if that entry is ours (it runs the relay script).
fn remove_status_line(text: &str) -> Result<String, String> {
    let start = text.find(KEY).ok_or("There's no status line to remove.")?;
    let open = start + text[start..].find('{').ok_or("Couldn't read the status line entry.")?;
    let end = object_end(text, open).ok_or("Couldn't read the status line entry.")?;
    if !text[start..end].contains(SCRIPT_NAME) {
        return Err("The status line in settings.json isn't TaskbarHWM's, so it was left alone.".into());
    }
    let (mut from, mut to) = (start, end);
    // Take the comma after the entry with it; if it was the last entry, take the comma before it instead.
    let after = text[to..].trim_start();
    if after.starts_with(',') {
        to = text.len() - after.len() + 1;
    } else if after.starts_with('}') {
        let before = text[..from].trim_end();
        if before.ends_with(',') {
            from = before.len() - 1;
        }
    }
    // And the line break and indentation that led up to it.
    let lead = text[..from].trim_end_matches([' ', '\t']);
    if lead.ends_with("\r\n") {
        from = lead.len() - 2;
    } else if lead.ends_with('\n') {
        from = lead.len() - 1;
    }
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..from]);
    out.push_str(&text[to..]);
    Ok(out)
}

/// Where Claude Code stands: not installed, free for us, busy with another status line, or connected.
pub fn link_state() -> Link {
    let dir = claude_dir();
    if !dir.is_dir() {
        return Link::NoClaudeCode;
    }
    match std::fs::read_to_string(dir.join("settings.json")) {
        Ok(text) if has_status_line(&text) => {
            if text.contains(SCRIPT_NAME) {
                Link::Connected
            } else {
                Link::OtherStatusLine
            }
        }
        _ => Link::NotConnected,
    }
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("json.taskbarhwm-tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("Couldn't write {} ({e}).", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't update {} ({e}).", path.display())
    })
}

fn write_script() -> Result<std::path::PathBuf, String> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {} ({e}).", dir.display()))?;
    let path = dir.join(SCRIPT_NAME);
    std::fs::write(&path, SCRIPT).map_err(|e| format!("Couldn't write the relay script ({e})."))?;
    Ok(path)
}

/// Keeps the installed relay script current after an app update.
pub fn refresh_script() {
    if link_state() == Link::Connected {
        let _ = write_script();
    }
}

/// Adds TaskbarHWM's status line to Claude Code's settings.json (keeping a one-time backup of the original).
pub fn connect() -> Result<(), String> {
    let dir = claude_dir();
    if !dir.is_dir() {
        return Err("Claude Code's folder wasn't found. Install and start Claude Code first.".into());
    }
    let script = write_script()?;
    let command = relay_command(&script);
    let settings = dir.join("settings.json");
    let text = match std::fs::read_to_string(&settings) {
        Ok(text) => {
            let backup = dir.join("settings.json.taskbarhwm-backup");
            if !backup.exists() {
                let _ = std::fs::write(&backup, &text);
            }
            insert_status_line(&text, &command)?
        }
        Err(_) => format!("{{\n  {KEY}: {{ \"type\": \"command\", \"command\": \"{}\" }}\n}}\n", json_escape(&command)),
    };
    write_atomic(&settings, &text)
}

/// Removes TaskbarHWM's status line again, along with the relay script and the data it saved.
pub fn disconnect() -> Result<(), String> {
    let settings = claude_dir().join("settings.json");
    let text = std::fs::read_to_string(&settings).map_err(|_| "Claude Code's settings.json wasn't found.".to_string())?;
    let new_text = remove_status_line(&text)?;
    write_atomic(&settings, &new_text)?;
    let dir = data_dir();
    let _ = std::fs::remove_file(dir.join(SCRIPT_NAME));
    let _ = std::fs::remove_file(dir.join("claude-status.txt"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: &str = "powershell -NoProfile -File \"C:/Users/X/AppData/Local/TaskbarHWM/claude-relay.ps1\"";

    #[test]
    fn adds_the_entry_and_keeps_the_rest() {
        let original = "{\n  \"model\": \"opus\",\n  \"env\": { \"A\": \"1\" }\n}\n";
        let out = insert_status_line(original, CMD).unwrap();
        assert!(out.starts_with("{\n  \"statusLine\": {"));
        assert!(out.contains("\"model\": \"opus\""));
        assert!(out.contains("\\\"C:/Users/X/AppData/Local/TaskbarHWM/claude-relay.ps1\\\""));
        // Still the same document otherwise: taking the entry out again restores it exactly.
        assert_eq!(remove_status_line(&out).unwrap(), original);
    }

    #[test]
    fn adds_to_an_empty_object_without_a_dangling_comma() {
        let out = insert_status_line("{}", CMD).unwrap();
        assert!(!out.contains(",\n}") && !out.contains("},}"), "{out}");
        assert!(out.ends_with("}") && out.contains("statusLine"));
        assert_eq!(remove_status_line(&out).unwrap(), "{}");
    }

    #[test]
    fn handles_a_byte_order_mark_and_refuses_non_objects() {
        assert!(insert_status_line("\u{feff}{\"a\":1}", CMD).is_ok());
        assert!(insert_status_line("[1,2]", CMD).is_err());
        assert!(insert_status_line("", CMD).is_err());
    }

    #[test]
    fn leaves_someone_elses_status_line_alone() {
        let theirs = "{\n  \"statusLine\": { \"type\": \"command\", \"command\": \"bash ~/x.sh\" }\n}";
        assert!(insert_status_line(theirs, CMD).is_err());
        assert!(remove_status_line(theirs).is_err());
        assert!(has_status_line(theirs));
    }

    #[test]
    fn removes_the_entry_when_it_is_not_the_first_key() {
        let t = format!("{{\n  \"a\": 1,\n  \"statusLine\": {{ \"type\": \"command\", \"command\": \"{}\" }},\n  \"b\": 2\n}}", json_escape(CMD));
        assert_eq!(remove_status_line(&t).unwrap(), "{\n  \"a\": 1,\n  \"b\": 2\n}");
        // ... and when it is the last one.
        let t = format!("{{\n  \"a\": 1,\n  \"statusLine\": {{ \"type\": \"command\", \"command\": \"{}\" }}\n}}", json_escape(CMD));
        assert_eq!(remove_status_line(&t).unwrap(), "{\n  \"a\": 1\n}");
    }
}
