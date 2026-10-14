//! Making toast the desktop's notifications, and giving them back.
//!
//! toast is started by systemd: at login, and by D-Bus whenever anything
//! is sent to it before then. On XFCE, xfce4-notifyd is kept from
//! starting, in ways `toast remove` undoes; on GNOME the shell keeps
//! the notifications, and toast's extension in it hands them on.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use crate::config::{self, config_home, data_home, short};
use crate::dbus::{Bus, Message};
use crate::server::{FDO, NAME};
use crate::style::{Style, GREEN, YELLOW};
use crate::{cli, extension};

const UNIT: &str = include_str!("../assets/toast.service");
const ICON: &str = include_str!("../assets/toast.svg");
const ENTRY: &str = include_str!("../assets/toast.desktop");

/// What toast leaves in a file it writes over another's: so that it is
/// only ever its own it takes away again.
const MARK: &str = "# Written by toast setup; toast remove takes it away.";

fn unit_path() -> PathBuf {
    config_home().join("systemd/user/toast.service")
}

/// The files that tell D-Bus to start toast for a name.
fn activation(name: &str) -> PathBuf {
    data_home().join(format!("dbus-1/services/{name}.service"))
}

fn autostart(name: &str) -> PathBuf {
    config_home().join(format!("autostart/{name}.desktop"))
}

fn menu_paths() -> (PathBuf, PathBuf) {
    (data_home().join("icons/hicolor/scalable/apps/toast.svg"), data_home().join("applications/toast.desktop"))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Takes away a file of toast's own, and says whether there was one.
fn unwrite(path: &Path) -> bool {
    let ours = fs::read_to_string(path).is_ok_and(|text| text.contains(MARK));
    ours && fs::remove_file(path).is_ok()
}

fn systemctl(arguments: &[&str]) -> bool {
    Command::new("systemctl").arg("--user").args(arguments).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn xfce() -> bool {
    env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| desktop.split(':').any(|d| d.eq_ignore_ascii_case("XFCE")))
}

/// Has the bus read its folders of services again, to find toast's.
fn reload_bus() {
    if let Ok(mut bus) = Bus::session() {
        let _ = bus.ask(Message::call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ReloadConfig", Vec::new()));
    }
}

/// Who has the notifications now, by the name of their program.
fn holder() -> Option<String> {
    let mut bus = Bus::session().ok()?;
    let owner = bus.owner(FDO)?;
    let asked = bus.ask(Message::call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetConnectionUnixProcessID", vec![crate::dbus::Value::text(&owner)])).ok()?;
    let pid = asked.body.first().and_then(crate::dbus::Value::as_i32)?;
    Some(fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_string())
}

pub fn setup() -> Result<(), String> {
    let style = Style::of_terminal();
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy();

    let settings = config::path();
    if settings.exists() {
        print!("{}", style.line(GREEN, "Unchanged", &short(&settings)));
    } else {
        write(&settings, config::EXAMPLE)?;
        print!("{}", style.line(GREEN, "Created", &short(&settings)));
    }

    // The icon, and the entry that ties it to toast's windows and puts
    // the history in the list of programs.
    let entry = ENTRY.replace("Exec=toast", &format!("Exec={exe}"));
    let (icon_at, entry_at) = menu_paths();
    write(&icon_at, ICON)?;
    write(&entry_at, &entry)?;
    print!("{}", style.line(GREEN, "Installed", "icon and menu entry"));

    // systemd runs it; D-Bus asks systemd to, for either name, before
    // it is running; and it is started at login.
    write(&unit_path(), &UNIT.replace("@EXE@", &exe))?;
    let service = |name: &str| format!("{MARK}\n[D-BUS Service]\nName={name}\nExec={exe} run\nSystemdService=toast.service\n");
    write(&activation(NAME), &service(NAME))?;
    let gnome = extension::gnome();
    if !gnome {
        write(&activation(FDO), &service(FDO))?;
    }
    let login = format!("{MARK}\n[Desktop Entry]\nType=Application\nName=toast\nComment=The desktop's notifications\nExec=systemctl --user start toast.service\nNoDisplay=true\n");
    write(&autostart("toast"), &login)?;
    reload_bus();
    print!("{}", style.line(GREEN, "Installed", "toast.service, started at login and by D-Bus"));

    // XFCE's own is kept from starting, at login and by D-Bus, and
    // stopped; `toast remove` undoes each.
    if xfce() || holder().as_deref() == Some("xfce4-notifyd") {
        let hidden = autostart("xfce4-notifyd");
        if !hidden.exists() {
            write(&hidden, &format!("{MARK}\n[Desktop Entry]\nType=Application\nName=Xfce Notification Daemon\nHidden=true\n"))?;
        }
        systemctl(&["mask", "--quiet", "xfce4-notifyd.service"]);
        systemctl(&["stop", "xfce4-notifyd.service"]);
        let _ = Command::new("pkill").args(["-x", "xfce4-notifyd"]).status();
        print!("{}", style.line(GREEN, "Stopped", "xfce4-notifyd, and kept from starting again"));
        print!("{}", style.under("its panel plugin, if you have it, has nothing to show now: take it off the panel"));
    }

    if gnome {
        if let Some((done, what)) = extension::install() {
            print!("{}", style.line(GREEN, done, what));
        }
    }

    systemctl(&["daemon-reload"]);
    if !systemctl(&["restart", "toast.service"]) {
        return Err("toast.service did not start: journalctl --user -u toast".into());
    }
    print!("{}", style.line(GREEN, "Started", "toast.service"));
    // Another notification server lets go in its own time.
    for _ in 0..20 {
        if matches!(holder().as_deref(), Some("toast") | Some("gnome-shell")) {
            break;
        }
        sleep(Duration::from_millis(150));
    }
    match holder().as_deref() {
        Some("toast") | Some("gnome-shell") => {}
        Some(other) => print!("{}", style.warning(&format!("{other} still has the notifications: stop it, and toast takes over"))),
        None => {}
    }
    println!();
    cli::status()
}

pub fn remove() -> Result<(), String> {
    let style = Style::of_terminal();
    systemctl(&["disable", "--quiet", "--now", "toast.service"]);
    let _ = fs::remove_file(unit_path());
    systemctl(&["daemon-reload"]);
    for path in [activation(NAME), activation(FDO), autostart("toast")] {
        unwrite(&path);
    }
    reload_bus();
    print!("{}", style.line(GREEN, "Removed", "toast.service"));
    if extension::gnome() || fs::metadata(data_home().join("gnome-shell/extensions/toast@jeffjose.github.io")).is_ok() {
        extension::remove();
        print!("{}", style.line(GREEN, "Removed", "GNOME extension (the shell shows its own again)"));
    }
    // XFCE's own, as it was.
    let unhid = unwrite(&autostart("xfce4-notifyd"));
    let unmasked = fs::read_link(config_home().join("systemd/user/xfce4-notifyd.service")).is_ok_and(|to| to == Path::new("/dev/null")) && systemctl(&["unmask", "--quiet", "xfce4-notifyd.service"]);
    if unhid || unmasked {
        systemctl(&["start", "xfce4-notifyd.service"]);
        print!("{}", style.line(GREEN, "Restored", "xfce4-notifyd"));
    }
    let (icon_at, entry_at) = menu_paths();
    if fs::remove_file(entry_at).is_ok() | fs::remove_file(icon_at).is_ok() {
        print!("{}", style.line(GREEN, "Removed", "icon and menu entry"));
    }
    print!("{}", style.line(YELLOW, "Kept", &format!("{} and {}", short(&config::path()), short(&crate::history::path()))));
    Ok(())
}
