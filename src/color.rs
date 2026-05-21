use std::io::IsTerminal;
use std::sync::OnceLock;

static USE_COLOR: OnceLock<bool> = OnceLock::new();

pub fn enabled() -> bool {
    *USE_COLOR.get_or_init(|| std::io::stderr().is_terminal())
}

macro_rules! ansi {
    ($code:expr, $text:expr) => {
        if $crate::color::enabled() {
            format!("\x1b[{}m{}\x1b[0m", $code, $text)
        } else {
            $text.to_string()
        }
    };
}

pub fn bold_green(text: &str) -> String {
    ansi!("1;32", text)
}

pub fn bold_yellow(text: &str) -> String {
    ansi!("1;33", text)
}

pub fn bold_cyan(text: &str) -> String {
    ansi!("1;36", text)
}

pub fn bold_white(text: &str) -> String {
    ansi!("1", text)
}

pub fn bold_blue(text: &str) -> String {
    ansi!("1;34", text)
}
