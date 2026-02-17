//! OSC 52 clipboard support (yank-to-clipboard via terminal escape sequences).
//!
//! Works over SSH and inside tmux by writing directly to `/dev/tty`.

use std::io::Write;

use base64::Engine;

/// Copy text to the system clipboard via OSC 52.
/// Automatically wraps for tmux if $TMUX is set.
pub fn osc52_copy(text: &str) -> std::io::Result<()> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let seq = format!("\x1b]52;c;{b64}\x07");
    let out = if std::env::var_os("TMUX").is_some() {
        let escaped = seq.replace('\x1b', "\x1b\x1b");
        format!("\x1bPtmux;\x1b{escaped}\x1b\\")
    } else {
        seq
    };
    let mut tty = std::fs::OpenOptions::new().write(true).open("/dev/tty")?;
    tty.write_all(out.as_bytes())?;
    tty.flush()
}
