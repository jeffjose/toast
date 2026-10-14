//! The history: every notification kept, a line of JSON each, in
//! ~/.local/state/toast/history, readable only by you. A notification
//! that replaces another is written again under the same number, and the
//! later line is the one that counts.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use crate::config;
use crate::note::Record;

pub fn path() -> PathBuf {
    if let Some(path) = std::env::var_os("TOAST_HISTORY") {
        return PathBuf::from(path);
    }
    config::state_home().join("toast/history")
}

fn open(append: bool) -> std::io::Result<fs::File> {
    let path = path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).mode(0o600);
    if append { options.append(true) } else { options.write(true).truncate(true) }.open(path)
}

pub fn add(record: &Record) {
    if let (Ok(mut file), Ok(line)) = (open(true), serde_json::to_string(record)) {
        let _ = writeln!(file, "{line}");
    }
}

/// Everything kept, the oldest first, each as it was last written.
pub fn all() -> Vec<Record> {
    let text = fs::read_to_string(path()).unwrap_or_default();
    let mut records: Vec<Record> = Vec::new();
    let mut at: HashMap<u32, usize> = HashMap::new();
    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<Record>(line) else { continue };
        match at.get(&record.id) {
            Some(&index) => records[index] = record,
            None => {
                at.insert(record.id, records.len());
                records.push(record);
            }
        }
    }
    records
}

/// The number the next notification is to have, which is one past any
/// kept, so that numbers are not used twice across restarts.
pub fn next_id() -> u32 {
    all().iter().map(|r| r.id).max().unwrap_or(0).wrapping_add(1).max(1)
}

fn write_all(records: &[Record]) {
    let Ok(mut file) = open(false) else { return };
    let text: String = records.iter().filter_map(|r| serde_json::to_string(r).ok()).map(|line| line + "\n").collect();
    let _ = file.write_all(text.as_bytes());
}

/// Keeps only the last `keep`, once there are well more than that, and
/// one line for each.
pub fn trim(keep: usize) {
    let lines = fs::read_to_string(path()).map_or(0, |text| text.lines().count());
    if lines > keep + keep / 2 + 10 {
        let records = all();
        write_all(&records[records.len().saturating_sub(keep)..]);
    }
}

/// Forgets one, or with none named, all of them.
pub fn forget(id: Option<u32>) -> usize {
    let records = all();
    let kept: Vec<Record> = records.iter().filter(|r| id.is_some_and(|id| r.id != id)).cloned().collect();
    write_all(&kept);
    records.len() - kept.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_replaced_and_forgotten() {
        let dir = std::env::temp_dir().join(format!("toast-history-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TOAST_HISTORY", dir.join("history"));
        let record = |id: u32, summary: &str| Record { id, summary: summary.into(), ..Record::default() };
        add(&record(1, "one"));
        add(&record(2, "two"));
        add(&record(1, "one, again"));
        let all = all();
        assert_eq!(all.iter().map(|r| r.summary.as_str()).collect::<Vec<_>>(), ["one, again", "two"]);
        assert_eq!(next_id(), 3);
        assert_eq!(forget(Some(1)), 1);
        assert_eq!(super::all().len(), 1);
        assert_eq!(forget(None), 1);
        assert_eq!(next_id(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
