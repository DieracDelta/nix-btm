//! OSC-52 yank-to-clipboard with optional async I/O.
//!
//! Defaults to sync I/O, but offers async-io feature flag
//! Only works on Linux/posix compliant systems only (uses /dev/tty)
//! compatible with tmux and ssh.

use std::{fmt::Formatter, io};

use base64::Engine;

const ESC: &str = "\x1b";
const DOUBLE_ESC: &str = "\x1b\x1b";
const BEL: &str = "\x07";

#[derive(Debug)]
pub enum Osc52Error {
    /// `/dev/tty` couldn't be opened for writing
    InaccessibleTty,
    /// couldn't write to `/dev/tty`
    Io(io::Error),
}

impl std::fmt::Display for Osc52Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Osc52Error::InaccessibleTty => {
                write!(f, "no controlling terminal (/dev/tty)")
            }
            Osc52Error::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for Osc52Error {}

impl From<io::Error> for Osc52Error {
    fn from(err: io::Error) -> Self {
        Osc52Error::Io(err)
    }
}

/// which clipboard to use
#[derive(Clone, Copy, Debug)]
pub enum Clipboard {
    System,
    Primary,
    Secondary,
    Custom(&'static str),
}

impl Clipboard {
    /// Return the identifier string for osc52
    fn selector(&self) -> &str {
        match self {
            Clipboard::System => "c",
            Clipboard::Primary => "p",
            Clipboard::Secondary => "q",
            Clipboard::Custom(s) => s,
        }
    }
}

/// Build an OSC 52 sequence for `text`
/// Uses the clipboard selection `clipboard`. Wraps for tmux if `TMUX` is set.
pub fn make_osc52_sequence(text: &str, clipboard: Clipboard) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let selector = clipboard.selector();

    let osc = format!("{ESC}]52;{selector};{b64}{BEL}");

    if std::env::var_os("TMUX").is_some() {
        let escaped = osc.replace(ESC, DOUBLE_ESC);
        format!("{ESC}Ptmux;{ESC}{escaped}{ESC}\\")
    } else {
        osc
    }
}

#[cfg(not(feature = "async-io"))]
fn write_to_tty_sync(bytes: &[u8]) -> Result<(), Osc52Error> {
    use std::{fs::OpenOptions, io::Write};

    if let Ok(mut tty) = OpenOptions::new().write(true).open("/dev/tty") {
        tty.write_all(bytes)?;
        tty.flush().map_err(|e| e.into())
    } else {
        Err(Osc52Error::InaccessibleTty)
    }
}

/// Copy `text` to the local clipboard via OSC 52
#[cfg(not(feature = "async-io"))]
pub fn osc52_copy(text: &str) -> Result<(), Osc52Error> {
    let seq = make_osc52_sequence(text, Clipboard::System);
    write_to_tty_sync(seq.as_bytes())
}

#[cfg(feature = "async-io")]
async fn write_to_tty_async(bytes: &[u8]) -> Result<(), Osc52Error> {
    use async_fs::OpenOptions;
    use futures_util::io::AsyncWriteExt;

    if let Ok(mut tty) = OpenOptions::new().write(true).open("/dev/tty").await {
        tty.write_all(bytes).await?;
        tty.flush().await.map_err(|e| e.into())
    } else {
        Err(Osc52Error::InaccessibleTty)
    }
}

/// Copy `text` to the local clipboard via OSC 52 (async)
#[cfg(feature = "async-io")]
pub async fn osc52_copy(text: &str) -> Result<(), Osc52Error> {
    let seq = make_osc52_sequence(text, Clipboard::System);
    write_to_tty_async(seq.as_bytes()).await
}
