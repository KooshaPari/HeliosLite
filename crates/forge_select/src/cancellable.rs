use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

/// A terminal answer retains choice indices instead of interpreting labels.
pub enum PromptAnswer {
    Text(String),
    Indices(Vec<usize>),
}

struct RawMode;
impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Reads a cancellable live prompt. Cancellation releases terminal input within
/// one polling interval, so a remote response cannot leave a hidden widget alive.
/// Choices are numbered from one for display and returned zero-based.
pub fn prompt_cancellable(
    message: &str,
    choices: &[String],
    multiple: bool,
    cancelled: Arc<AtomicBool>,
) -> anyhow::Result<Option<PromptAnswer>> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Ok(None);
    }
    let mut output = std::io::stderr();
    writeln!(output, "\n{message}")?;
    for (index, choice) in choices.iter().enumerate() {
        writeln!(output, "  {}. {choice}", index + 1)?;
    }
    if !choices.is_empty() {
        writeln!(
            output,
            "Enter {} (Esc cancels).",
            if multiple {
                "choice numbers separated by commas"
            } else {
                "one choice number"
            }
        )?;
    }
    crossterm::terminal::enable_raw_mode()?;
    let _raw = RawMode;
    write!(output, "> ")?;
    output.flush()?;
    let mut text = String::new();
    while !cancelled.load(Ordering::Acquire) {
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        // Recheck after polling: never consume another prompt's input on cancel.
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(None);
            }
            KeyCode::Enter => {
                write!(output, "\r\n")?;
                if choices.is_empty() {
                    return Ok(Some(PromptAnswer::Text(text)));
                }
                let parsed = text
                    .split(',')
                    .filter(|part| !part.trim().is_empty())
                    .map(|part| {
                        part.trim()
                            .parse::<usize>()
                            .ok()
                            .and_then(|value| value.checked_sub(1))
                    })
                    .collect::<Option<Vec<_>>>();
                if let Some(indices) = parsed {
                    if (multiple || indices.len() == 1)
                        && indices.iter().enumerate().all(|(offset, index)| {
                            *index < choices.len() && !indices[..offset].contains(index)
                        })
                    {
                        return Ok(Some(PromptAnswer::Indices(indices)));
                    }
                }
                text.clear();
                write!(output, "Invalid choice. > ")?;
            }
            KeyCode::Backspace => {
                if text.pop().is_some() {
                    write!(output, "\x08 \x08")?;
                }
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                text.push(character);
                write!(output, "{character}")?;
            }
            _ => {}
        }
        output.flush()?;
    }
    write!(output, "\r\n")?;
    Ok(None)
}
