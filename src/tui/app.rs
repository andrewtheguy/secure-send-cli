//! Wizard state machine: collects everything a transfer needs (direction,
//! selection, signaling mode, output directory, PIN) before any network work.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Paragraph;

use crate::crypto::pin::{
    PIN_LENGTH, compute_pin_fingerprint, format_pin_fingerprint, is_pin_char, is_valid_pin,
};

use super::file_browser::{Browser, BrowserStep};
use super::is_ctrl_c;
use super::widgets;

/// The resolved outcome of the wizard: what to transfer and how.
pub enum WizardPlan {
    /// Stays in the TUI.
    SendNostr(Vec<PathBuf>),
    /// Leaves the TUI so the SS03 blobs can be copy/pasted.
    SendManual(Vec<PathBuf>),
    /// Stays in the TUI.
    ReceiveNostr { pin: String, output: PathBuf },
    /// Leaves the TUI so the SS03 blobs can be copy/pasted.
    ReceiveManual { output: PathBuf },
}

const MODE_ITEMS: &[&str] = &[
    "PIN code via Nostr relays (works with secure-send-web Auto Exchange)",
    "Manual copy/paste exchange (leaves this screen for the code swap)",
];

enum Screen {
    MainMenu {
        selected: usize,
    },
    FileBrowser(Browser),
    SendMode {
        browser: Browser,
        selected: usize,
    },
    ReceiveMode {
        selected: usize,
    },
    OutputDir {
        manual: bool,
        input: String,
        error: Option<String>,
    },
    PinEntry {
        output: PathBuf,
        input: String,
        fingerprint: Option<String>,
        error: Option<String>,
    },
}

enum Step {
    Continue(Screen),
    Finish(WizardPlan),
    Quit,
}

/// Run the wizard. `Ok(None)` means the user quit cleanly.
pub async fn run_wizard(terminal: &mut DefaultTerminal) -> Result<Option<WizardPlan>> {
    let mut screen = Screen::MainMenu { selected: 0 };
    let mut events = EventStream::new();

    loop {
        terminal.draw(|f| draw(f, &mut screen))?;

        let event = events
            .next()
            .await
            .ok_or_else(|| anyhow!("input stream closed"))??;
        let Event::Key(key) = event else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if is_ctrl_c(&key) {
            return Err(anyhow!("Interrupted"));
        }

        match handle_key(screen, key) {
            Step::Continue(next) => screen = next,
            Step::Finish(plan) => return Ok(Some(plan)),
            Step::Quit => return Ok(None),
        }
    }
}

fn handle_key(screen: Screen, key: KeyEvent) -> Step {
    match screen {
        Screen::MainMenu { selected } => main_menu_key(selected, key),
        Screen::FileBrowser(mut browser) => match browser.handle_key(key) {
            BrowserStep::Stay => Step::Continue(Screen::FileBrowser(browser)),
            BrowserStep::Back => Step::Continue(Screen::MainMenu { selected: 0 }),
            BrowserStep::Confirm => Step::Continue(Screen::SendMode {
                browser,
                selected: 0,
            }),
        },
        Screen::SendMode { browser, selected } => send_mode_key(browser, selected, key),
        Screen::ReceiveMode { selected } => receive_mode_key(selected, key),
        Screen::OutputDir {
            manual,
            input,
            error,
        } => output_dir_key(manual, input, error, key),
        Screen::PinEntry {
            output,
            input,
            fingerprint,
            error,
        } => pin_entry_key(output, input, fingerprint, error, key),
    }
}

fn menu_move(selected: usize, len: usize, key: &KeyEvent) -> usize {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => (selected + 1).min(len - 1),
        _ => selected,
    }
}

fn main_menu_key(selected: usize, key: KeyEvent) -> Step {
    match key.code {
        KeyCode::Enter => match selected {
            0 => match Browser::new() {
                Ok(browser) => Step::Continue(Screen::FileBrowser(browser)),
                Err(_) => Step::Continue(Screen::MainMenu { selected }),
            },
            1 => Step::Continue(Screen::ReceiveMode { selected: 0 }),
            _ => Step::Quit,
        },
        KeyCode::Esc | KeyCode::Char('q') => Step::Quit,
        _ => Step::Continue(Screen::MainMenu {
            selected: menu_move(selected, 3, &key),
        }),
    }
}

fn send_mode_key(browser: Browser, selected: usize, key: KeyEvent) -> Step {
    match key.code {
        KeyCode::Enter => {
            let paths = browser.selection();
            if selected == 0 {
                Step::Finish(WizardPlan::SendNostr(paths))
            } else {
                Step::Finish(WizardPlan::SendManual(paths))
            }
        }
        KeyCode::Esc => Step::Continue(Screen::FileBrowser(browser)),
        _ => Step::Continue(Screen::SendMode {
            browser,
            selected: menu_move(selected, MODE_ITEMS.len(), &key),
        }),
    }
}

fn receive_mode_key(selected: usize, key: KeyEvent) -> Step {
    match key.code {
        KeyCode::Enter => Step::Continue(Screen::OutputDir {
            manual: selected == 1,
            input: default_output_dir(),
            error: None,
        }),
        KeyCode::Esc => Step::Continue(Screen::MainMenu { selected: 1 }),
        _ => Step::Continue(Screen::ReceiveMode {
            selected: menu_move(selected, MODE_ITEMS.len(), &key),
        }),
    }
}

fn default_output_dir() -> String {
    std::env::current_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|_| ".".to_string())
}

fn output_dir_key(manual: bool, mut input: String, error: Option<String>, key: KeyEvent) -> Step {
    match key.code {
        KeyCode::Enter => {
            let dir = PathBuf::from(input.trim());
            if input.trim().is_empty() {
                return Step::Continue(Screen::OutputDir {
                    manual,
                    input,
                    error: Some("Enter a directory".to_string()),
                });
            }
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return Step::Continue(Screen::OutputDir {
                    manual,
                    input,
                    error: Some(format!("Cannot create {}: {e}", dir.display())),
                });
            }
            if manual {
                Step::Finish(WizardPlan::ReceiveManual { output: dir })
            } else {
                Step::Continue(Screen::PinEntry {
                    output: dir,
                    input: String::new(),
                    fingerprint: None,
                    error: None,
                })
            }
        }
        KeyCode::Esc => Step::Continue(Screen::ReceiveMode {
            selected: usize::from(manual),
        }),
        KeyCode::Backspace => {
            input.pop();
            Step::Continue(Screen::OutputDir {
                manual,
                input,
                error: None,
            })
        }
        KeyCode::Char(c) => {
            input.push(c);
            Step::Continue(Screen::OutputDir {
                manual,
                input,
                error: None,
            })
        }
        _ => Step::Continue(Screen::OutputDir {
            manual,
            input,
            error,
        }),
    }
}

fn pin_entry_key(
    output: PathBuf,
    mut input: String,
    mut fingerprint: Option<String>,
    error: Option<String>,
    key: KeyEvent,
) -> Step {
    match key.code {
        KeyCode::Enter => {
            if is_valid_pin(&input) {
                Step::Finish(WizardPlan::ReceiveNostr { pin: input, output })
            } else {
                Step::Continue(Screen::PinEntry {
                    output,
                    input,
                    fingerprint,
                    error: Some("Invalid PIN: check for typos and try again".to_string()),
                })
            }
        }
        KeyCode::Esc => Step::Continue(Screen::OutputDir {
            manual: false,
            input: output.display().to_string(),
            error: None,
        }),
        KeyCode::Backspace => {
            input.pop();
            Step::Continue(Screen::PinEntry {
                output,
                input,
                fingerprint: None,
                error: None,
            })
        }
        KeyCode::Char(c) if is_pin_char(c) && input.len() < PIN_LENGTH => {
            input.push(c);
            if input.len() == PIN_LENGTH && is_valid_pin(&input) {
                // One-time PBKDF2 (~200k iterations); worth the beat for the
                // visual check against the sender's fingerprint.
                fingerprint = Some(format_pin_fingerprint(&compute_pin_fingerprint(&input)));
            }
            Step::Continue(Screen::PinEntry {
                output,
                input,
                fingerprint,
                error: None,
            })
        }
        _ => Step::Continue(Screen::PinEntry {
            output,
            input,
            fingerprint,
            error,
        }),
    }
}

fn draw(f: &mut Frame, screen: &mut Screen) {
    match screen {
        Screen::MainMenu { selected } => {
            let inner = widgets::screen_frame(f, "wizard");
            let area = widgets::centered(inner, 40, 6);
            let [title, _, list] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .areas(area);
            f.render_widget(Paragraph::new("What do you want to do?"), title);
            widgets::menu(
                f,
                list,
                &["Send files or a folder", "Receive", "Quit"],
                *selected,
            );
            widgets::key_hints(f, inner, "↑/↓ move · Enter select · q quit");
        }

        Screen::FileBrowser(browser) => {
            let inner = widgets::screen_frame(f, "send");
            browser.render(f, inner);
        }

        Screen::SendMode { browser, selected } => {
            let inner = widgets::screen_frame(f, "send");
            let area = widgets::centered(inner, 74, 7);
            let [title, _, list] = Layout::vertical([
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .areas(area);
            let paths = browser.selection();
            f.render_widget(
                Paragraph::new(format!(
                    "Sending \"{}\" — how should the two sides connect?",
                    crate::archive::send_display_name(&paths)
                )),
                title,
            );
            widgets::menu(f, list, MODE_ITEMS, *selected);
            widgets::key_hints(f, inner, "↑/↓ move · Enter select · Esc back");
        }

        Screen::ReceiveMode { selected } => {
            let inner = widgets::screen_frame(f, "receive");
            let area = widgets::centered(inner, 74, 6);
            let [title, _, list] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .areas(area);
            f.render_widget(Paragraph::new("How is the sender sharing the transfer?"), title);
            widgets::menu(f, list, MODE_ITEMS, *selected);
            widgets::key_hints(f, inner, "↑/↓ move · Enter select · Esc back");
        }

        Screen::OutputDir {
            input, error, ..
        } => {
            let inner = widgets::screen_frame(f, "receive");
            let area = widgets::centered(inner, inner.width.saturating_sub(4).max(40), 4);
            let [title, line, err] = Layout::vertical([
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(area);
            f.render_widget(
                Paragraph::new("Where should the received file be saved?"),
                title,
            );
            widgets::input_line(f, line, "Output directory: ", input);
            if let Some(error) = error {
                widgets::error_line(f, err, error);
            }
            widgets::key_hints(f, inner, "type to edit · Enter confirm · Esc back");
        }

        Screen::PinEntry {
            input,
            fingerprint,
            error,
            ..
        } => {
            let inner = widgets::screen_frame(f, "receive");
            let area = widgets::centered(inner, 60, 5);
            let [title, line, extra] = Layout::vertical([
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(2),
            ])
            .areas(area);
            f.render_widget(
                Paragraph::new("Enter the sender's 12-character PIN:"),
                title,
            );
            widgets::input_line(f, line, "PIN: ", input);
            if let Some(error) = error {
                widgets::error_line(f, extra, error);
            } else if let Some(fp) = fingerprint {
                f.render_widget(
                    Paragraph::new(format!(
                        "PIN fingerprint: {fp} (should match the sender's)"
                    )),
                    extra,
                );
            }
            widgets::key_hints(f, inner, "Enter confirm · Esc back");
        }
    }
}
