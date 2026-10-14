//! toast as the desktop's notification server: the session bus's
//! org.freedesktop.Notifications, which every program that notifies
//! sends to, and toast's own name, which `toast` itself and GNOME's
//! shell speak to.
//!
//! The name every program sends to may already be someone else's: on
//! XFCE `toast setup` sees that it is not, and on GNOME the shell keeps
//! it and its part of toast hands on what it is sent. toast answers the
//! same either way, under its own name too.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;

use winit::event_loop::EventLoopProxy;

use crate::dbus::{self, Bus, Message, Value, Writer, CALL, SIGNAL};
use crate::note::Note;
use crate::{history, stack};

pub const FDO: &str = "org.freedesktop.Notifications";
pub const FDO_PATH: &str = "/org/freedesktop/Notifications";
pub const NAME: &str = "io.github.jeffjose.Toast";
pub const PATH: &str = "/io/github/jeffjose/Toast";

/// What the server tells the window side.
#[derive(Debug)]
pub enum Event {
    Note(Box<Note>),
    Close(u32),
    Dismiss(Option<u32>),
    Panel,
    Quiet(bool),
    Forgot,
}

/// What both sides know.
pub struct Shared {
    /// Whether the name every program sends to is ours.
    pub serving: AtomicBool,
    pub quiet: AtomicBool,
    next: AtomicU32,
    /// How many are on the screen, as the window side last counted.
    pub showing: AtomicU32,
}

const CAPABILITIES: [&str; 6] = ["actions", "body", "body-markup", "icon-static", "persistence", "x-toast"];

const FDO_INTERFACE: &str = r#"<interface name="org.freedesktop.Notifications">
  <method name="GetCapabilities"><arg type="as" direction="out"/></method>
  <method name="Notify"><arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/><arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/></method>
  <method name="CloseNotification"><arg type="u" direction="in"/></method>
  <method name="GetServerInformation"><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/></method>
  <signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
  <signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
</interface>"#;

const TOAST_INTERFACE: &str = r#"<interface name="io.github.jeffjose.Toast">
  <method name="Status"><arg type="s" direction="out"/></method>
  <method name="Panel"/>
  <method name="Dismiss"><arg type="u" direction="in"/></method>
  <method name="Quiet"><arg type="b" direction="in"/></method>
  <method name="Forget"><arg type="u" direction="in"/><arg type="u" direction="out"/></method>
</interface>"#;

/// Whether notifications are kept off the screen, as it was left.
fn quiet_path() -> std::path::PathBuf {
    crate::config::state_home().join("toast/quiet")
}

pub fn was_quiet() -> bool {
    quiet_path().exists()
}

fn set_quiet(quiet: bool) {
    let path = quiet_path();
    match quiet {
        true => {
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
            let _ = std::fs::write(&path, "");
        }
        false => drop(std::fs::remove_file(&path)),
    }
}

/// Takes toast's name, and the one every program sends to if it can be
/// had, and answers on a thread of its own from then on. Gives what the
/// window side needs to say things on the bus, and what both know.
pub fn start(proxy: EventLoopProxy<Event>) -> Result<(Writer, Arc<Shared>), String> {
    let mut bus = Bus::session().map_err(|e| e.to_string())?;
    match bus.request_name(NAME, dbus::DO_NOT_QUEUE).map_err(|e| e.to_string())? {
        1 | 4 => {}
        _ => return Err("toast is already running (toast status)".into()),
    }
    // Ours now, or when whoever has it lets go; and another's if a later
    // toast asks for it, which is how a new one takes over from an old.
    let serving = bus.request_name(FDO, dbus::ALLOW_REPLACEMENT | dbus::REPLACE_EXISTING).map_err(|e| e.to_string())? == 1;
    let shared = Arc::new(Shared { serving: AtomicBool::new(serving), quiet: AtomicBool::new(was_quiet()), next: AtomicU32::new(history::next_id()), showing: AtomicU32::new(0) });
    let writer = bus.writer();
    let both = shared.clone();
    thread::spawn(move || {
        while let Ok(Some(message)) = bus.next(None) {
            if !answer(&mut bus, &message, &both, &proxy) {
                break;
            }
        }
        // The bus gone, or the window side: nothing more to do.
        std::process::exit(0);
    });
    Ok((writer, shared))
}

/// Answers one message; false once the window side is gone.
fn answer(bus: &mut Bus, message: &Message, shared: &Shared, proxy: &EventLoopProxy<Event>) -> bool {
    let send = |event: Event| proxy.send_event(event).is_ok();
    if message.kind == SIGNAL && message.interface == "org.freedesktop.DBus" {
        let name = message.body.first().and_then(Value::as_str);
        match (message.member.as_str(), name) {
            ("NameAcquired", Some(FDO)) => shared.serving.store(true, Ordering::Relaxed),
            ("NameLost", Some(FDO)) => shared.serving.store(false, Ordering::Relaxed),
            _ => {}
        }
        return true;
    }
    if message.kind != CALL {
        return true;
    }
    let body = &message.body;
    let replied = match (message.interface.as_str(), message.member.as_str()) {
        ("org.freedesktop.DBus.Introspectable", "Introspect") => bus.answer(message, vec![Value::text(&introspect(&message.path))]),
        ("org.freedesktop.DBus.Peer", "Ping") => bus.answer(message, Vec::new()),
        (FDO | "", "GetCapabilities") => bus.answer(message, vec![Value::Array("s".into(), CAPABILITIES.map(Value::text).to_vec())]),
        (FDO | "", "GetServerInformation") => {
            let said = ["toast", "jeffjose", env!("CARGO_PKG_VERSION"), "1.2"];
            bus.answer(message, said.map(Value::text).to_vec())
        }
        (FDO | "", "Notify") => match Note::from_call(body) {
            Some(mut note) => {
                note.id = match note.replaces {
                    0 => shared.next.fetch_add(1, Ordering::Relaxed),
                    id => id,
                };
                let id = note.id;
                if !send(Event::Note(Box::new(note))) {
                    return false;
                }
                bus.answer(message, vec![Value::U32(id)])
            }
            None => bus.refuse(message, "Notify takes (susssasa{sv}i)"),
        },
        (FDO | "", "CloseNotification") => {
            if let Some(id) = body.first().and_then(Value::as_i32) {
                if !send(Event::Close(id as u32)) {
                    return false;
                }
            }
            bus.answer(message, Vec::new())
        }
        (NAME, "Status") => {
            let status = serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "serving": shared.serving.load(Ordering::Relaxed),
                "owner": if shared.serving.load(Ordering::Relaxed) { None } else { bus.owner(FDO) },
                "quiet": shared.quiet.load(Ordering::Relaxed),
                "showing": shared.showing.load(Ordering::Relaxed),
                "kept": history::all().len(),
                "placing": if std::env::var_os("WAYLAND_DISPLAY").is_some() { "wayland" } else { "x11" },
            });
            bus.answer(message, vec![Value::text(&status.to_string())])
        }
        (NAME, "Panel") => {
            if !send(Event::Panel) {
                return false;
            }
            bus.answer(message, Vec::new())
        }
        (NAME, "Dismiss") => {
            let id = body.first().and_then(Value::as_i32).unwrap_or(0) as u32;
            if !send(Event::Dismiss((id != 0).then_some(id))) {
                return false;
            }
            bus.answer(message, Vec::new())
        }
        (NAME, "Quiet") => {
            let quiet = matches!(body.first(), Some(Value::Bool(true)));
            shared.quiet.store(quiet, Ordering::Relaxed);
            set_quiet(quiet);
            if !send(Event::Quiet(quiet)) {
                return false;
            }
            bus.answer(message, Vec::new())
        }
        (NAME, "Forget") => {
            let id = body.first().and_then(Value::as_i32).unwrap_or(0) as u32;
            let id = (id != 0).then_some(id);
            let forgotten = history::forget(id);
            if !send(Event::Forgot) {
                return false;
            }
            bus.answer(message, vec![Value::U32(forgotten as u32)])
        }
        _ => bus.refuse(message, &format!("toast has no {}.{}", message.interface, message.member)),
    };
    replied.is_ok()
}

fn introspect(path: &str) -> String {
    let head = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">"#;
    let inner = match path {
        FDO_PATH => format!("{FDO_INTERFACE}\n{TOAST_INTERFACE}"),
        PATH => TOAST_INTERFACE.to_string(),
        "/" => r#"<node name="org"/>"#.to_string(),
        "/org" => r#"<node name="freedesktop"/>"#.to_string(),
        "/org/freedesktop" => r#"<node name="Notifications"/>"#.to_string(),
        "/io" => r#"<node name="github"/>"#.to_string(),
        "/io/github" => r#"<node name="jeffjose"/>"#.to_string(),
        "/io/github/jeffjose" => r#"<node name="Toast"/>"#.to_string(),
        _ => String::new(),
    };
    format!("{head}\n<node>\n{inner}\n</node>\n")
}

/// Tells whoever listens that an action was chosen.
pub fn invoked(writer: &Writer, id: u32, key: &str) {
    let _ = writer.send(&Message::signal(FDO_PATH, FDO, "ActionInvoked", vec![Value::U32(id), Value::text(key)]));
}

/// Tells whoever listens that a notification has gone, and why.
pub fn closed(writer: &Writer, id: u32, reason: stack::Reason) {
    let _ = writer.send(&Message::signal(FDO_PATH, FDO, "NotificationClosed", vec![Value::U32(id), Value::U32(reason as u32)]));
}

/// Asks the running toast something under its own name.
pub fn ask(member: &str, body: Vec<Value>) -> Result<Vec<Value>, String> {
    let mut bus = Bus::session().map_err(|e| e.to_string())?;
    let asked = bus.ask(Message::call(NAME, PATH, NAME, member, body));
    match asked {
        Ok(reply) => Ok(reply.body),
        Err(e) if e.to_string().contains("ServiceUnknown") || e.to_string().contains("NameHasNoOwner") => {
            Err("toast is not running (toast setup, or systemctl --user start toast)".into())
        }
        Err(e) => Err(e.to_string()),
    }
}
