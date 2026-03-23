use std::env;
use std::fmt::Display;
use std::io::IsTerminal;

use owo_colors::OwoColorize;

/// Crate / CLI version (from Cargo.toml).
pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns true if we should use colors on stdout (TTY and NO_COLOR not set).
pub fn color_stdout() -> bool {
    !env::var("NO_COLOR").is_ok() && std::io::stdout().is_terminal()
}

/// Returns true if we should use colors on stderr (TTY and NO_COLOR not set).
pub fn color_stderr() -> bool {
    !env::var("NO_COLOR").is_ok() && std::io::stderr().is_terminal()
}

/// Prints a warning to stderr, in yellow when stderr is a TTY.
pub fn warn(msg: impl Display) {
    let s = msg.to_string();
    if color_stderr() {
        eprintln!("{}", s.yellow());
    } else {
        eprintln!("{}", s);
    }
}

/// Prints an error to stderr with a consistent prefix (red when stderr is a TTY).
pub fn error(msg: impl Display) {
    let s = msg.to_string();
    if color_stderr() {
        eprintln!("{} {}", "Error:".red().bold(), s);
    } else {
        eprintln!("Error: {}", s);
    }
}

/// Apply stdout style when colors are enabled; otherwise return plain text.
pub fn style_stdout<F>(msg: &str, style: F) -> String
where
    F: FnOnce(&str) -> String,
{
    if color_stdout() {
        style(msg)
    } else {
        msg.to_string()
    }
}

// --- Theme helpers (stdout): roles for labels, data, state ---

/// Main title line (e.g. help header).
pub fn title_app(color: bool) -> String {
    if color {
        "memkit CLI".bold().cyan().to_string()
    } else {
        "memkit CLI".to_string()
    }
}

/// Section heading (doctor, models).
pub fn section_title(color: bool, s: &str) -> String {
    if color {
        s.bold().cyan().to_string()
    } else {
        s.to_string()
    }
}

/// Binary name accent (`mk`).
pub fn mk_binary(color: bool) -> String {
    if color {
        "mk".cyan().to_string()
    } else {
        "mk".to_string()
    }
}

/// Subcommand or keyword emphasis (bold).
pub fn bold_word(color: bool, s: &str) -> String {
    if color {
        s.bold().to_string()
    } else {
        s.to_string()
    }
}

/// Secondary / options / filler text.
pub fn dimmed_word(color: bool, s: &str) -> String {
    if color {
        s.dimmed().to_string()
    } else {
        s.to_string()
    }
}

/// Pack path in list/status (white when colors on).
pub fn white_word(color: bool, s: &str) -> String {
    if color {
        s.white().to_string()
    } else {
        s.to_string()
    }
}

/// Cyan parenthetical label e.g. `(default)` for pack list lines.
pub fn cyan_label(color: bool, s: &str) -> String {
    if color {
        s.cyan().to_string()
    } else {
        s.to_string()
    }
}

/// Bracketed detail for `mk doctor` lines, e.g. `[http://127.0.0.1:4242]` (cyan when color on).
pub fn bracketed_cyan(color: bool, inner: &str) -> String {
    let s = format!("[{}]", inner);
    if color {
        s.cyan().to_string()
    } else {
        s
    }
}

/// Metrics, counts, host:port (cyan).
pub fn data_num(color: bool, s: impl Display) -> String {
    let t = s.to_string();
    if color {
        t.cyan().to_string()
    } else {
        t
    }
}

/// Model id in query banners (magenta).
pub fn magenta_words(color: bool, s: &str) -> String {
    if color {
        s.magenta().to_string()
    } else {
        s.to_string()
    }
}

/// Active `[local]` / `[cloud]` tags in pack list (cyan when on).
pub fn cyan_words(color: bool, s: &str) -> String {
    if color {
        s.cyan().to_string()
    } else {
        s.to_string()
    }
}

pub fn success_words(color: bool, s: &str) -> String {
    if color {
        s.green().to_string()
    } else {
        s.to_string()
    }
}

pub fn warn_words(color: bool, s: &str) -> String {
    if color {
        s.yellow().to_string()
    } else {
        s.to_string()
    }
}

pub fn danger_words(color: bool, s: &str) -> String {
    if color {
        s.red().to_string()
    } else {
        s.to_string()
    }
}

/// Path + success state (bold green line prefix).
pub fn bold_green(color: bool, s: &str) -> String {
    if color {
        s.bold().green().to_string()
    } else {
        s.to_string()
    }
}

/// Path + warning state (bold yellow).
pub fn bold_yellow(color: bool, s: &str) -> String {
    if color {
        s.bold().yellow().to_string()
    } else {
        s.to_string()
    }
}

/// `sync: local only` (dimmed).
pub fn sync_local_only_label(color: bool) -> String {
    dimmed_word(color, "sync: local only")
}
