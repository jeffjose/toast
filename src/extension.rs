//! The part of toast that lives in GNOME's shell, where on Wayland
//! windows are placed from, and where the shell's notifications are:
//! putting it in, taking it out, and asking it how it is. clutch's, for
//! toast.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const UUID: &str = "toast@jeffjose.github.io";
const PATH: &str = "/io/github/jeffjose/Toast/Shell";
const NAME: &str = "io.github.jeffjose.Toast.Shell";

/// Which writing of the extension this is. The shell keeps running the
/// one it started with, and says which that was.
const VERSION: u32 = 1;

/// What it answers to, and what it hands on and to whom.
const CONSTANTS: &str = r#"const VERSION = 1;
const TOAST = 'io.github.jeffjose.Toast';
const FDO = 'org.freedesktop.Notifications';
const PATH = '/org/freedesktop/Notifications';
const SHELL_INTERFACE = `<node><interface name="io.github.jeffjose.Toast.Shell">
  <method name="State"><arg type="s" direction="out"/></method>
</interface></node>`;
const FDO_INTERFACE = `<node><interface name="org.freedesktop.Notifications">
  <method name="GetCapabilities"><arg type="as" direction="out"/></method>
  <method name="Notify"><arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/><arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/></method>
  <method name="CloseNotification"><arg type="u" direction="in"/></method>
  <method name="GetServerInformation"><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/></method>
  <signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
  <signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
</interface></node>`;
"#;

/// The same extension is written two ways: the shell took to modules
/// with version 45, and reads nothing else since.
const MODULES: &str = "import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
";
const BEFORE_MODULES: &str = "const {Gio, GLib, Meta} = imports.gi;
const Main = imports.ui.main;
";
const BODY: &str = include_str!("../assets/extension.js");

fn dir() -> PathBuf {
    crate::config::data_home().join("gnome-shell/extensions").join(UUID)
}

/// Whether this is GNOME's shell.
pub fn gnome() -> bool {
    env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| desktop.split(':').any(|d| d == "GNOME"))
}

/// The shell's version, its first number: 46.
fn shell_version() -> Option<u32> {
    let output = Command::new("gnome-shell").arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace().find_map(|word| word.split('.').next()?.parse().ok())
}

fn source(version: u32) -> String {
    match version >= 45 {
        true => format!("{MODULES}\n{CONSTANTS}\nexport default class Toast extends Extension {{\n{BODY}}}\n"),
        false => format!("{BEFORE_MODULES}\n{CONSTANTS}\nclass Toast {{\n{BODY}}}\n\nfunction init() {{\n    return new Toast();\n}}\n"),
    }
}

fn gsettings(arguments: &[&str]) -> Option<String> {
    let output = Command::new("gsettings").args(arguments).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The strings in what gsettings prints: ['a', 'b'] gives a and b.
fn strings(printed: &str) -> Vec<String> {
    printed.split('\'').skip(1).step_by(2).map(str::to_string).collect()
}

fn list(strings: &[String]) -> String {
    match strings.is_empty() {
        true => "@as []".to_string(),
        false => format!("[{}]", strings.iter().map(|s| format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))).collect::<Vec<_>>().join(", ")),
    }
}

/// Put it in and turn it on. A shell on Wayland only finds a new
/// extension when it starts, which is at the next login. Returns what
/// was done and to what, or `None` where there is no GNOME shell.
pub fn install() -> Option<(&'static str, &'static str)> {
    let version = shell_version()?;
    let dir = dir();
    fs::create_dir_all(&dir).ok()?;
    let metadata = format!(
        "{{\n  \"uuid\": \"{UUID}\",\n  \"name\": \"toast\",\n  \"description\": \"Puts toast's notifications in their corner, and hands toast the shell's notifications.\",\n  \
         \"url\": \"https://github.com/jeffjose/toast\",\n  \"shell-version\": [\"{version}\"]\n}}\n"
    );
    fs::write(dir.join("metadata.json"), metadata).ok()?;
    fs::write(dir.join("extension.js"), source(version)).ok()?;
    enable(true);
    // A shell that already knew of it takes a moment to start it.
    let mut running = None;
    for _ in 0..10 {
        running = state();
        if running.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    Some(match running {
        Some(state) if state.contains(&format!("\"version\":{VERSION},")) => ("Loaded", "GNOME extension"),
        Some(_) => ("Updated", "GNOME extension (log out and in to load the new one)"),
        None => ("Installed", "GNOME extension (log out and in once to load it)"),
    })
}

pub fn remove() {
    enable(false);
    let _ = fs::remove_dir_all(dir());
}

/// Put it on the shell's list of extensions to run, or take it off. The
/// list is written to directly: `gnome-extensions enable` asks the shell,
/// which has not heard of an extension put in since it started and so
/// leaves it off, to stay off at the next login as well.
fn enable(on: bool) {
    const LIST: [&str; 2] = ["org.gnome.shell", "enabled-extensions"];
    let Some(printed) = gsettings(&["get", LIST[0], LIST[1]]) else { return };
    let mut enabled = strings(&printed);
    if enabled.iter().any(|uuid| uuid == UUID) == on {
        return;
    }
    enabled.retain(|uuid| uuid != UUID);
    if on {
        enabled.push(UUID.to_string());
    }
    let _ = gsettings(&["set", LIST[0], LIST[1], &list(&enabled)]);
}

/// What the running extension says of itself, as JSON.
pub fn state() -> Option<String> {
    let output = Command::new("gdbus")
        .args(["call", "--session", "--dest", "org.gnome.Shell", "--object-path", PATH, "--method"])
        .arg(format!("{NAME}.State"))
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // The one string gdbus printed, as it was sent: ('…',) without its
    // wrapping.
    let printed = String::from_utf8_lossy(&output.stdout);
    let inner = printed.trim().strip_prefix("('")?.strip_suffix("',)")?;
    Some(inner.replace("\\\\", "\\"))
}

/// Whether the shell has this extension running, and if not, why not,
/// as nearly as can be told from outside the shell.
pub fn health() -> Result<String, String> {
    if !dir().join("extension.js").exists() {
        return Err("not installed: run `toast setup`".into());
    }
    if let Some(state) = state() {
        return match state.contains(&format!("\"version\":{VERSION},")) {
            true => Ok(state),
            false => Err("an older one is loaded: log out and in".into()),
        };
    }
    let enabled = gsettings(&["get", "org.gnome.shell", "enabled-extensions"]);
    if enabled.is_some_and(|list| !strings(&list).iter().any(|uuid| uuid == UUID)) {
        return Err("switched off: run `toast setup`".into());
    }
    let info = Command::new("gnome-extensions").args(["info", UUID]).output();
    let info = info.map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default();
    let said = |label: &str| info.lines().find_map(|line| line.trim().strip_prefix(label).map(|rest| rest.trim().to_string()));
    match (said("State:"), said("Error:")) {
        (Some(state), Some(error)) => Err(format!("the shell has it as {state}: {error}")),
        (Some(state), None) if state != "ACTIVE" && state != "ENABLED" => Err(format!("the shell has it as {state}")),
        (None, _) => Err("the shell has not found it yet: log out and in".into()),
        _ => Err("loaded, but it does not answer".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_both_ways() {
        let new = source(46);
        assert!(new.starts_with("import Gio from 'gi://Gio';"));
        assert!(new.contains("export default class Toast extends Extension {"));
        assert!(!new.contains("function init()"));
        let old = source(42);
        assert!(old.starts_with("const {Gio, GLib, Meta} = imports.gi;"));
        assert!(old.contains("function init()") && !old.contains("import "));
        for source in [&new, &old] {
            assert!(source.contains("<method name=\"State\">") && source.contains("    State() {"));
        }
        // Written out where asked, for a JavaScript engine to check.
        if let Some(dir) = env::var_os("TOAST_EXTENSION_TO") {
            let dir = PathBuf::from(dir);
            fs::write(dir.join("new.mjs"), new).unwrap();
            fs::write(dir.join("old.js"), old).unwrap();
        }
    }

    #[test]
    fn the_list_of_extensions() {
        let printed = "['clutch@jeffjose.github.io', 'x@y']\n";
        assert_eq!(strings(printed), ["clutch@jeffjose.github.io", "x@y"]);
        assert_eq!(list(&strings(printed)), "['clutch@jeffjose.github.io', 'x@y']");
        assert_eq!(list(&[]), "@as []");
    }
}
