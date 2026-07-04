//! Plain-text terminal output and interactive prompts for the CLI.
//!
//! Status/progress go to stderr; the base64 signaling codes the user must copy
//! go to stdout so they can be piped or redirected cleanly.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow};

use crate::util::{calc_percent, format_bytes};

/// Direction of a transfer, used to label progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Send,
    Receive,
}

/// User's choice when a destination file already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileExistsChoice {
    Overwrite,
    Rename,
    Cancel,
}

/// Informational status line (stderr).
pub fn status(line: &str) {
    eprintln!("{line}");
}

/// Informational status line with elapsed time.
pub fn status_timed(line: &str, elapsed: Duration) {
    eprintln!("{line} ({})", format_elapsed(elapsed));
}

fn format_elapsed(elapsed: Duration) -> String {
    let ms = elapsed.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", elapsed.as_secs_f64())
    }
}

/// A base64 signaling code the user must copy (stdout, framed for readability).
pub fn show_code(title: &str, code: &str) {
    println!("\n----- {title} -----");
    println!("{code}");
    println!("----- end -----\n");
    let _ = std::io::stdout().flush();
}

/// Update the single-line live progress indicator (stderr).
pub fn progress(dir: Direction, bytes: u64, total: u64) {
    let verb = match dir {
        Direction::Send => "Sending",
        Direction::Receive => "Receiving",
    };
    eprint!(
        "\r   {verb}: {}% ({}/{})",
        calc_percent(bytes, total) as u32,
        format_bytes(bytes),
        format_bytes(total),
    );
    let _ = std::io::stderr().flush();
}

/// Terminate the live progress line with a newline.
pub fn progress_end() {
    eprintln!();
}

/// Ask how to handle an existing destination file.
pub fn prompt_file_exists(path: &Path) -> Result<FileExistsChoice> {
    print!(
        "Warning: file exists: {}\n[o]verwrite / [r]ename / [c]ancel: ",
        path.display()
    );
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    match input.trim().to_lowercase().as_str() {
        "o" | "overwrite" => Ok(FileExistsChoice::Overwrite),
        "r" | "rename" => Ok(FileExistsChoice::Rename),
        _ => Ok(FileExistsChoice::Cancel),
    }
}

/// Read a line of input from the user with line editing.
pub fn prompt_line(prompt: &str) -> Result<String> {
    use rustyline::DefaultEditor;

    let mut rl = DefaultEditor::new().map_err(|e| anyhow!(e.to_string()))?;
    match rl.readline(prompt) {
        Ok(line) => Ok(line),
        Err(rustyline::error::ReadlineError::Interrupted) => Err(anyhow!("Interrupted")),
        Err(rustyline::error::ReadlineError::Eof) => Err(anyhow!("EOF")),
        Err(e) => Err(anyhow!(e.to_string())),
    }
}

/// Read a potentially multi-line pasted code, terminated by a blank line or EOF.
///
/// Base64 SS03 codes are single-line, but users may paste with wrapping; we
/// accumulate lines until a blank line so wrapped pastes still work.
pub fn prompt_multiline(prompt: &str) -> Result<String> {
    use std::io::BufRead;

    eprintln!("{prompt}");
    eprintln!("(paste the code, then press Enter on an empty line)");
    let stdin = std::io::stdin();
    let mut collected = String::new();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            if collected.trim().is_empty() {
                continue; // ignore leading blank lines
            }
            break;
        }
        collected.push_str(line.trim());
    }
    if collected.trim().is_empty() {
        return Err(anyhow!("no code entered"));
    }
    Ok(collected)
}
