//! toast's settings, from ~/.config/toast/config.toml: where the
//! notifications show, how long they stay, and rules for particular ones.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::note::{Corner, Kind, Note, CRITICAL, LOW};

/// The settings written for a new install: every one, at what it is
/// when not said.
pub const EXAMPLE: &str = r#"# toast's settings. They are read again whenever a notification comes.

# Where notifications show: top-left, top, top-right, bottom-left,
# bottom or bottom-right.
position = "bottom-right"

# How long one stays, in seconds, unless it says otherwise. One that
# matters (critical) stays until it is dismissed. The pointer over the
# stack holds them all.
seconds = 6

# How many show at once. More wait behind them, and come forward as
# those in front go.
stack = 3

# How many are kept in the history.
keep = 1000

# Rules, for particular programs or notifications. The first that
# matches has its say. A rule matches on any of app, summary, body,
# category and urgency ("low", "normal", "critical"), where * in the
# words matches anything, and letters match either case. It can say
# show (false: kept in the history, not shown), sticky, seconds,
# position, kind ("success", "info", "warning", "error") and keep
# (false: not kept in the history either).
#
# [[rule]]
# app = "Spotify"
# show = false
#
# [[rule]]
# summary = "*backup*"
# sticky = true
# position = "top"
"#;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub position: String,
    pub seconds: f64,
    pub stack: usize,
    pub keep: usize,
    #[serde(rename = "rule")]
    pub rules: Vec<Rule>,
}

impl Default for Config {
    fn default() -> Config {
        Config { position: "bottom-right".into(), seconds: 6.0, stack: 3, keep: 1000, rules: Vec::new() }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub app: Option<String>,
    pub summary: Option<String>,
    pub body: Option<String>,
    pub category: Option<String>,
    pub urgency: Option<String>,
    pub show: Option<bool>,
    pub sticky: Option<bool>,
    pub seconds: Option<f64>,
    pub position: Option<String>,
    pub kind: Option<String>,
    pub keep: Option<bool>,
}

/// What toast is to do with a notification, its settings and rules
/// taken together.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Treatment {
    pub show: bool,
    pub keep: bool,
    pub corner: Corner,
    /// How long it stays, in milliseconds; none for until dismissed.
    pub lasts: Option<u64>,
    pub kind: Kind,
}

impl Config {
    /// Settings from their text, or what is wrong with it.
    pub fn parse(text: &str) -> Result<Config, String> {
        let config: Config = toml::from_str(text).map_err(|e| e.to_string().trim().to_string())?;
        if Corner::named(&config.position).is_none() {
            return Err(format!("position: no such place as \"{}\"", config.position));
        }
        for (at, rule) in config.rules.iter().enumerate() {
            if let Some(position) = rule.position.as_deref().filter(|p| Corner::named(p).is_none()) {
                return Err(format!("rule {}: position: no such place as \"{position}\"", at + 1));
            }
            if let Some(kind) = rule.kind.as_deref().filter(|k| Kind::named(k).is_none()) {
                return Err(format!("rule {}: kind: no such kind as \"{kind}\"", at + 1));
            }
        }
        Ok(config)
    }

    pub fn treat(&self, note: &Note) -> Treatment {
        let rule = self.rules.iter().find(|rule| rule.matches(note));
        let said = |pick: fn(&Rule) -> Option<bool>| rule.and_then(pick);
        let sticky = note.sticky || note.timeout == 0 || note.urgency == CRITICAL || said(|r| r.sticky) == Some(true);
        let seconds = rule.and_then(|r| r.seconds).unwrap_or(self.seconds);
        let lasts = match (sticky, note.timeout) {
            (true, _) => None,
            // A program's own say, unless a rule says otherwise.
            (false, ms) if ms > 0 && rule.and_then(|r| r.seconds).is_none() => Some(ms as u64),
            _ => Some((seconds.max(0.5) * 1000.0) as u64),
        };
        let corner = note.corner.or_else(|| Corner::named(rule?.position.as_deref()?)).or(Corner::named(&self.position)).unwrap_or_default();
        let kind = rule.and_then(|r| Kind::named(r.kind.as_deref()?)).unwrap_or(note.kind);
        let keep = !note.transient && said(|r| r.keep) != Some(false);
        Treatment { show: !note.quiet && said(|r| r.show) != Some(false), keep, corner, lasts, kind }
    }
}

impl Rule {
    fn matches(&self, note: &Note) -> bool {
        let urgency = match note.urgency {
            LOW => "low",
            CRITICAL => "critical",
            _ => "normal",
        };
        let tests = [(&self.app, note.app.as_str()), (&self.summary, &note.summary), (&self.body, &note.body), (&self.category, &note.category), (&self.urgency, urgency)];
        tests.iter().any(|(pattern, _)| pattern.is_some()) && tests.iter().all(|(pattern, text)| pattern.as_deref().is_none_or(|p| matches(p, text)))
    }
}

/// Whether `text` is what `pattern` says, where * is anything at all and
/// letters are either case.
pub fn matches(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.to_lowercase(), text.to_lowercase());
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || !text[first.len()..].ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// The settings as they are on disk, read again when the file changes.
pub struct Settings {
    pub path: PathBuf,
    pub config: Config,
    written: Option<SystemTime>,
    /// What was wrong with the file when it was last read, if anything:
    /// the settings before it stay.
    pub problem: Option<String>,
}

impl Settings {
    pub fn load(path: PathBuf) -> Settings {
        let mut settings = Settings { path, config: Config::default(), written: None, problem: None };
        settings.fresh();
        settings
    }

    /// Reads the file again if it changed since it was read.
    pub fn fresh(&mut self) -> &Config {
        let written = fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if written != self.written {
            self.written = written;
            match fs::read_to_string(&self.path) {
                Ok(text) => match Config::parse(&text) {
                    Ok(config) => (self.config, self.problem) = (config, None),
                    Err(problem) => self.problem = Some(problem),
                },
                Err(_) => (self.config, self.problem) = (Config::default(), None),
            }
        }
        &self.config
    }
}

fn home() -> PathBuf {
    PathBuf::from(env::var_os("HOME").unwrap_or_default())
}

pub fn state_home() -> PathBuf {
    match env::var_os("XDG_STATE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home().join(".local/state"),
    }
}

pub fn config_home() -> PathBuf {
    match env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home().join(".config"),
    }
}

pub fn data_home() -> PathBuf {
    match env::var_os("XDG_DATA_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home().join(".local/share"),
    }
}

/// A path as it is shortest to read: under ~ where it is.
pub fn short(path: &Path) -> String {
    match path.strip_prefix(home()) {
        Ok(rest) if env::var_os("HOME").is_some() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

pub fn path() -> PathBuf {
    config_home().join("toast/config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_is_the_defaults() {
        let config = Config::parse(EXAMPLE).unwrap();
        let plain = Config::default();
        assert_eq!((config.position, config.seconds, config.stack, config.keep), (plain.position, plain.seconds, plain.stack, plain.keep));
        assert!(config.rules.is_empty());
        // The rules shown, taken out of their comments, are rules.
        let rules: String = EXAMPLE.lines().filter(|l| l.starts_with("# [[") || l.starts_with("# ") && l.contains(" = ")).map(|l| format!("{}\n", &l[2..])).collect();
        assert_eq!(Config::parse(&rules).unwrap().rules.len(), 2);
    }

    #[test]
    fn what_is_wrong_is_said() {
        assert!(Config::parse("position = \"middle\"").unwrap_err().contains("middle"));
        assert!(Config::parse("colour = 1").is_err());
        assert!(Config::parse("[[rule]]\napp = \"x\"\nkind = \"odd\"").unwrap_err().contains("rule 1"));
    }

    #[test]
    fn words_with_stars() {
        assert!(matches("*backup*", "Nightly Backup done"));
        assert!(matches("spotify", "Spotify"));
        assert!(!matches("spotify", "Spotify Free"));
        assert!(matches("a*c", "abc") && matches("a*c", "ac") && !matches("a*c", "ab"));
        assert!(matches("*", ""));
    }

    #[test]
    fn rules_have_their_say() {
        let config = Config::parse("seconds = 4\n[[rule]]\napp = \"spot*\"\nshow = false\n[[rule]]\nsummary = \"*backup*\"\nposition = \"top\"\nsticky = true\n").unwrap();
        let note = |app: &str, summary: &str| Note { app: app.into(), summary: summary.into(), timeout: -1, urgency: 1, ..Note::default() };
        let plain = config.treat(&note("Firefox", "Download done"));
        assert_eq!((plain.show, plain.corner, plain.lasts), (true, Corner::BottomRight, Some(4000)));
        assert!(!config.treat(&note("Spotify", "Song")).show);
        let backup = config.treat(&note("rsync", "Backup finished"));
        assert_eq!((backup.corner, backup.lasts), (Corner::Top, None));
        // What a program asks for, unless a rule says.
        assert_eq!(config.treat(&Note { timeout: 1500, ..note("x", "y") }).lasts, Some(1500));
        assert_eq!(config.treat(&Note { urgency: CRITICAL, ..note("x", "y") }).lasts, None);
    }
}
