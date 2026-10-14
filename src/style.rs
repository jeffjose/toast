//! How toast speaks on a terminal: as cargo does. A word for what was
//! done or how something stands, bold and colored, set to the right of
//! a column, and then the thing itself.

use std::env;

pub const GREEN: &str = "32";
pub const CYAN: &str = "36";
pub const YELLOW: &str = "33";
pub const RED: &str = "31";

/// How wide the column of words is.
pub const VERB: usize = 12;

/// The words of one piece of output, in color or not.
#[derive(Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    /// In color when what is written to is a terminal, unless asked not to.
    pub fn of_terminal() -> Style {
        let terminal = unsafe { libc::isatty(1) } == 1;
        Style { color: terminal && env::var_os("NO_COLOR").is_none() }
    }

    fn paint(&self, codes: &str, text: &str) -> String {
        match self.color {
            true => format!("\x1b[{codes}m{text}\x1b[0m"),
            false => text.to_string(),
        }
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }

    pub fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }

    /// "    Finished the thing": the word in its color, then the rest.
    pub fn line(&self, color: &str, verb: &str, text: &str) -> String {
        format!("{} {text}\n", self.paint(&format!("1;{color}"), &format!("{verb:>VERB$}")))
    }

    /// A line under another, saying more about it.
    pub fn under(&self, text: &str) -> String {
        format!("{:VERB$} {text}\n", "")
    }

    pub fn warning(&self, text: &str) -> String {
        format!("{}{} {text}\n", self.paint(&format!("1;{YELLOW}"), "warning"), self.bold(":"))
    }

    pub fn error(&self, text: &str) -> String {
        format!("{}{} {text}\n", self.paint(&format!("1;{RED}"), "error"), self.bold(":"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_word_to_the_right_of_its_column() {
        let plain = Style { color: false };
        assert_eq!(plain.line(GREEN, "Bound", "ctrl+space"), "       Bound ctrl+space\n");
        assert_eq!(plain.under("why"), "             why\n");
        assert_eq!(plain.warning("careful"), "warning: careful\n");
        let color = Style { color: true };
        assert_eq!(color.line(GREEN, "Bound", "x"), "\x1b[1;32m       Bound\x1b[0m x\n");
    }
}
