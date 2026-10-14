//! What `toast` does from a terminal or a script: send a notification,
//! read the history, and ask the running toast how it is.

use std::time::Duration;

use crate::dbus::{Bus, Message, Value, SIGNAL};
use crate::note::{Corner, Kind, Note};
use crate::server::{self, FDO, FDO_PATH};
use crate::style::{Style, CYAN, GREEN, YELLOW};
use crate::{config, demo, history, panel};

/// The words of an action, made into what the program is told: "Open
/// folder" is "open-folder".
fn key_of(label: &str) -> String {
    let key: String = label.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    key.trim_matches('-').to_string()
}

/// A notification as a call to Notify.
fn call(note: &Note) -> Message {
    let mut hints = Vec::new();
    let mut hint = |name: &str, value: Value| hints.push(Value::Entry(Box::new(Value::text(name)), Box::new(Value::any(value))));
    hint("urgency", Value::Byte(note.urgency));
    if note.kind != Kind::Plain {
        hint("x-toast-kind", Value::text(&format!("{:?}", note.kind).to_lowercase()));
    }
    if let Some(corner) = note.corner {
        hint("x-toast-where", Value::text(corner.name()));
    }
    if note.sticky {
        hint("x-toast-sticky", Value::Bool(true));
    }
    if note.quiet {
        hint("x-toast-quiet", Value::Bool(true));
    }
    if let Some(progress) = note.progress {
        hint("value", Value::I32(progress));
    }
    if !note.run.is_empty() {
        let lines: String = note.run.iter().map(|(key, line)| format!("{key}\t{line}\n")).collect();
        hint("x-toast-run", Value::text(&lines));
    }
    let actions: Vec<Value> = note.actions.iter().flat_map(|(key, label)| [Value::text(key), Value::text(label)]).collect();
    Message::call(
        FDO,
        FDO_PATH,
        FDO,
        "Notify",
        vec![
            Value::text(&note.app),
            Value::U32(note.replaces),
            Value::text(&note.icon),
            Value::text(&note.summary),
            Value::text(&note.body),
            Value::Array("s".into(), actions),
            Value::Array("{sv}".into(), hints),
            Value::I32(note.timeout),
        ],
    )
}

pub const SEND_HELP: &str = "\
toast send SUMMARY [BODY] [options]

  --app NAME          who it is from (toast)
  --icon NAME|FILE    an icon's name, or a picture
  --kind KIND         success, info, warning or error: an icon and a color
  --urgency LEVEL     low, normal or critical (critical ones stay)
  --seconds N         how long it stays
  --sticky            it stays until dismissed
  --where PLACE       top-left, top, top-right, bottom-left, bottom, bottom-right
  --progress N        a bar, N of 100 along
  --quiet             kept in the history, not shown
  --replace ID        in place of one sent before
  --action LABEL[=COMMAND]
                      a button; with a command, toast runs it when clicked
  --click COMMAND     run when the notification itself is clicked
  --wait              wait for a button or a click, and print its key
  --print-id          print the number it was given, for --replace
";

pub fn send(arguments: Vec<String>) -> Result<(), String> {
    let mut note = Note { app: "toast".into(), urgency: 1, timeout: -1, ..Note::default() };
    let (mut wait, mut print_id) = (false, false);
    let mut words = Vec::new();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        let mut value = || arguments.next().ok_or(format!("{argument} needs a value"));
        match argument.as_str() {
            "--app" => note.app = value()?,
            "--icon" => {
                let icon = value()?;
                // A file is said in full, wherever it was named from.
                note.icon = match std::fs::canonicalize(&icon) {
                    Ok(path) if icon.contains('/') || icon.contains('.') => path.display().to_string(),
                    _ => icon,
                };
            }
            "--kind" => {
                let kind = value()?;
                note.kind = Kind::named(&kind).ok_or(format!("--kind: no kind {kind}: success, info, warning or error"))?;
            }
            "--urgency" => {
                note.urgency = match value()?.as_str() {
                    "low" => 0,
                    "normal" => 1,
                    "critical" => 2,
                    other => return Err(format!("--urgency: low, normal or critical, not {other}")),
                }
            }
            "--seconds" => {
                let seconds: f64 = value()?.parse().map_err(|_| "--seconds: a number")?;
                note.timeout = (seconds * 1000.0).max(1.0) as i32;
            }
            "--sticky" => note.sticky = true,
            "--quiet" => note.quiet = true,
            "--where" => {
                let place = value()?;
                note.corner = Some(Corner::named(&place).ok_or(format!("--where: no such place as {place}"))?);
            }
            "--progress" => note.progress = Some(value()?.parse::<i32>().map_err(|_| "--progress: a number")?.clamp(0, 100)),
            "--replace" => note.replaces = value()?.parse().map_err(|_| "--replace: a number")?,
            "--action" => {
                let given = value()?;
                let (label, command) = match given.split_once('=') {
                    Some((label, command)) => (label.to_string(), Some(command.to_string())),
                    None => (given, None),
                };
                let key = key_of(&label);
                if let Some(command) = command {
                    note.run.push((key.clone(), command));
                }
                note.actions.push((key, label));
            }
            "--click" => {
                note.run.push(("default".into(), value()?));
                note.actions.push(("default".into(), "Open".into()));
            }
            "--wait" => wait = true,
            "--print-id" => print_id = true,
            "-h" | "--help" => {
                print!("{SEND_HELP}");
                return Ok(());
            }
            flag if flag.starts_with("--") => return Err(format!("send: no option {flag} (toast send --help)")),
            _ => words.push(argument),
        }
    }
    let mut words = words.into_iter();
    note.summary = words.next().ok_or("send: what to say (toast send --help)")?;
    note.body = words.collect::<Vec<_>>().join(" ");
    // Waiting for an action where there is none would be forever.
    if wait && note.actions.is_empty() {
        note.actions.push(("default".into(), "Open".into()));
    }
    let mut bus = Bus::session().map_err(|e| e.to_string())?;
    if wait {
        bus.listen(&format!("type='signal',interface='{FDO}'")).map_err(|e| e.to_string())?;
    }
    let reply = bus.ask(call(&note)).map_err(|e| format!("no notification server answered: {e}"))?;
    let id = reply.body.first().and_then(Value::as_i32).unwrap_or(0) as u32;
    if print_id {
        println!("{id}");
    }
    if !wait {
        return Ok(());
    }
    loop {
        let Some(message) = bus.next(None).map_err(|e| e.to_string())? else { continue };
        if message.kind != SIGNAL || message.body.first().and_then(Value::as_i32) != Some(id as i32) {
            continue;
        }
        match message.member.as_str() {
            "ActionInvoked" => {
                println!("{}", message.body.get(1).and_then(Value::as_str).unwrap_or(""));
                return Ok(());
            }
            "NotificationClosed" => std::process::exit(1),
            _ => {}
        }
    }
}

/// Sends the demo's notifications, one after another, to watch them come.
pub fn demo() -> Result<(), String> {
    let mut bus = Bus::session().map_err(|e| e.to_string())?;
    for note in demo::notes() {
        bus.ask(call(&note)).map_err(|e| format!("no notification server answered: {e}"))?;
        std::thread::sleep(Duration::from_millis(700));
    }
    Ok(())
}

/// The last `count` notifications kept, the newest last, as lines.
pub fn log(arguments: Vec<String>) -> Result<(), String> {
    let json = arguments.iter().any(|a| a == "--json");
    let count = arguments.iter().find_map(|a| a.parse::<usize>().ok()).unwrap_or(20);
    let all = history::all();
    let shown = &all[all.len().saturating_sub(count)..];
    if json {
        for record in shown {
            println!("{}", serde_json::to_string(record).map_err(|e| e.to_string())?);
        }
        return Ok(());
    }
    let style = Style::of_terminal();
    let width = shown.iter().map(|r| r.app.chars().count()).max().unwrap_or(0).min(16);
    let mut last_day = String::new();
    for record in shown {
        let day = panel::day_of(record.time);
        if day != last_day {
            println!("{}", style.bold(&day));
            last_day = day;
        }
        let app: String = record.app.chars().take(16).collect();
        let body = record.body.lines().next().unwrap_or("");
        let body = if body.is_empty() { String::new() } else { style.dim(&format!("  {body}")) };
        println!("  {}  {app:width$}  {}{body}", style.dim(&panel::time_of(record.time)), record.summary);
    }
    if shown.is_empty() {
        println!("{}", style.dim("nothing kept yet"));
    }
    Ok(())
}

/// What process has a name on the bus, by what the system calls it.
fn program_of(bus: &mut Bus, name: &str) -> Option<String> {
    let asked = bus.ask(Message::call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetConnectionUnixProcessID", vec![Value::text(name)])).ok()?;
    let pid = asked.body.first().and_then(Value::as_i32)?;
    Some(std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_string())
}

pub fn status() -> Result<(), String> {
    let style = Style::of_terminal();
    let mut bus = Bus::session().map_err(|e| e.to_string())?;
    let said = match server::ask("Status", Vec::new()) {
        Ok(said) => said,
        Err(problem) => {
            print!("{}", style.line(YELLOW, "Stopped", &problem));
            if let Some(owner) = bus.owner(FDO) {
                let program = program_of(&mut bus, &owner).unwrap_or(owner);
                print!("{}", style.line(CYAN, "Serving", &format!("notifications: {program}")));
            }
            return Ok(());
        }
    };
    let status: serde_json::Value = serde_json::from_str(said.first().and_then(Value::as_str).unwrap_or("{}")).map_err(|e| e.to_string())?;
    let text = |name: &str| status[name].as_str().unwrap_or("").to_string();
    print!("{}", style.line(GREEN, "Running", &format!("toast {}, placing windows for {}", text("version"), text("placing"))));
    if status["serving"].as_bool() == Some(true) {
        print!("{}", style.line(GREEN, "Serving", "org.freedesktop.Notifications"));
    } else {
        let owner = text("owner");
        let program = program_of(&mut bus, &owner).unwrap_or(owner);
        let how = match program.as_str() {
            "gnome-shell" => "GNOME's shell has it, and hands what it is sent to toast through its extension".to_string(),
            _ => format!("{program} has it; toast takes over when it lets go (toast setup)"),
        };
        print!("{}", style.line(YELLOW, "Waiting", &format!("for org.freedesktop.Notifications: {how}")));
    }
    print!("{}", style.line(GREEN, "Showing", &format!("{} on the screen", status["showing"].as_u64().unwrap_or(0))));
    print!("{}", style.line(GREEN, "Kept", &format!("{} in {}", status["kept"].as_u64().unwrap_or(0), config::short(&history::path()))));
    if crate::extension::gnome() {
        match crate::extension::health() {
            Ok(state) if state.contains("\"forwarding\":true") => print!("{}", style.line(GREEN, "Extension", "places toast's windows, and hands it the shell's notifications")),
            Ok(_) => print!("{}", style.line(YELLOW, "Extension", "places toast's windows, but cannot find the shell's notifications to hand on")),
            Err(why) => print!("{}", style.line(YELLOW, "Extension", &why)),
        }
    }
    if status["quiet"].as_bool() == Some(true) {
        print!("{}", style.line(YELLOW, "Quiet", "only critical ones are shown (toast quiet off)"));
    }
    let settings = config::Settings::load(config::path());
    if let Some(problem) = settings.problem {
        print!("{}", style.warning(&format!("{}: {problem}", config::short(&settings.path))));
    }
    Ok(())
}

pub fn quiet(which: Option<&str>) -> Result<(), String> {
    let now = match server::ask("Status", Vec::new())?.first().and_then(Value::as_str).map(serde_json::from_str::<serde_json::Value>) {
        Some(Ok(status)) => status["quiet"].as_bool().unwrap_or(false),
        _ => false,
    };
    let quiet = match which {
        Some("on") => true,
        Some("off") => false,
        None | Some("toggle") => !now,
        Some(other) => return Err(format!("quiet: on, off or toggle, not {other}")),
    };
    server::ask("Quiet", vec![Value::Bool(quiet)])?;
    let style = Style::of_terminal();
    print!("{}", match quiet {
        true => style.line(YELLOW, "Quiet", "only critical notifications are shown; the rest are kept"),
        false => style.line(GREEN, "Unquiet", "notifications are shown"),
    });
    Ok(())
}

/// An id, or "all", or nothing for all.
fn which(argument: Option<&str>) -> Result<u32, String> {
    match argument {
        None | Some("all") => Ok(0),
        Some(id) => id.parse().map_err(|_| format!("a notification's number, or all, not {id}")),
    }
}

pub fn dismiss(argument: Option<&str>) -> Result<(), String> {
    server::ask("Dismiss", vec![Value::U32(which(argument)?)]).map(drop)
}

pub fn forget(argument: Option<&str>) -> Result<(), String> {
    if argument.is_none() {
        return Err("forget: a notification's number, or all".into());
    }
    let id = which(argument)?;
    // Without a toast running, the history is the file, as it is anyway.
    let forgotten = match server::ask("Forget", vec![Value::U32(id)]) {
        Ok(said) => said.first().and_then(Value::as_i32).unwrap_or(0) as usize,
        Err(_) => history::forget((id != 0).then_some(id)),
    };
    let style = Style::of_terminal();
    print!("{}", style.line(GREEN, "Forgot", &format!("{forgotten} notification{}", if forgotten == 1 { "" } else { "s" })));
    Ok(())
}

pub fn check(path: Option<String>) -> Result<(), String> {
    let path = path.map_or_else(config::path, std::path::PathBuf::from);
    let style = Style::of_terminal();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            print!("{}", style.line(GREEN, "Default", &format!("settings: there are none at {}", config::short(&path))));
            return Ok(());
        }
    };
    let config = config::Config::parse(&text).map_err(|e| format!("{}: {e}", config::short(&path)))?;
    print!("{}", style.line(GREEN, "Read", &config::short(&path)));
    print!("{}", style.under(&format!("at the {}, for {} seconds, {} at once, {} kept", config.position, config.seconds, config.stack, config.keep)));
    for rule in &config.rules {
        let mut matched = Vec::new();
        for (name, value) in [("app", &rule.app), ("summary", &rule.summary), ("body", &rule.body), ("category", &rule.category), ("urgency", &rule.urgency)] {
            if let Some(value) = value {
                matched.push(format!("{name} {value:?}"));
            }
        }
        print!("{}", style.line(CYAN, "Rule", &matched.join(", ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_from_labels() {
        assert_eq!(key_of("Open folder"), "open-folder");
        assert_eq!(key_of(" Reply! "), "reply");
    }

    #[test]
    fn a_notification_sent_is_the_one_read() {
        let mut note = Note { app: "test".into(), summary: "Hi".into(), body: "there".into(), urgency: 2, timeout: 3000, sticky: true, progress: Some(40), ..Note::default() };
        note.kind = Kind::Warning;
        note.corner = Some(Corner::Top);
        note.actions = vec![("open".into(), "Open".into())];
        note.run = vec![("open".into(), "xdg-open .".into())];
        let message = call(&note);
        let read = Note::from_call(&message.body).unwrap();
        assert_eq!((read.app, read.summary, read.body, read.urgency, read.timeout), (note.app, note.summary, note.body, 2, 3000));
        assert_eq!((read.kind, read.corner, read.sticky, read.progress), (Kind::Warning, Some(Corner::Top), true, Some(40)));
        assert_eq!((read.actions, read.run), (note.actions, note.run));
    }
}
