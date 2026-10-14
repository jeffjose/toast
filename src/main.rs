mod canvas;
mod cli;
mod config;
mod dbus;
mod demo;
mod extension;
mod history;
mod icons;
mod note;
mod panel;
mod server;
mod setup;
mod stack;
mod style;
mod ui;
mod x11;

use std::env;
use std::process::exit;

const HELP: &str = "\
toast: notifications drawn in the corner, kept, and sent from anywhere.

  toast send SUMMARY [BODY]     send a notification (toast send --help)
  toast history                 the history, in a box to search: Enter copies one
  toast log [N] [--json]        the last N kept, here
  toast dismiss [ID|all]        take them off the screen
  toast quiet [on|off]          show only critical ones, and keep the rest
  toast forget ID|all           take them out of the history
  toast demo                    send a few, to see what they look like

  toast setup     make toast this desktop's notifications, and start it
  toast status    what is running, and whether it has the notifications
  toast check     read the settings and say what is wrong with them
  toast remove    give the notifications back to the desktop
  toast run       what the service runs: be the notification server, here
  toast picture FILE [one|stack|expanded|panel]
                  draw the demo to a PNG, without a desktop
";

fn main() {
    let mut arguments = env::args().skip(1);
    let command = arguments.next().unwrap_or_default();
    let rest: Vec<String> = arguments.collect();
    let first = rest.first().map(String::as_str);
    let done = match command.as_str() {
        "run" => ui::run(),
        "send" => cli::send(rest),
        "history" => server::ask("Panel", Vec::new()).map(drop),
        "log" => cli::log(rest),
        "dismiss" => cli::dismiss(first),
        "quiet" => cli::quiet(first),
        "forget" => cli::forget(first),
        "demo" => cli::demo(),
        "status" => cli::status(),
        "check" => cli::check(rest.first().cloned()),
        "setup" => setup::setup(),
        "remove" => setup::remove(),
        "picture" => match first {
            Some(path) => demo::picture(path, rest.get(1).map_or("stack", String::as_str)),
            None => Err("picture FILE [one|stack|expanded|panel]".into()),
        },
        "" | "help" | "-h" | "--help" => {
            print!("{HELP}");
            Ok(())
        }
        "-V" | "--version" => {
            println!("toast {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(format!("no command {other} (toast --help)")),
    };
    if let Err(problem) = done {
        eprint!("{}", style::Style::of_terminal().error(&problem));
        exit(1);
    }
}
