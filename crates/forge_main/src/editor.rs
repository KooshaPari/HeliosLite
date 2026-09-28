use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use console::{measure_text_width, strip_ansi_codes};
use forge_api::Environment;
use nu_ansi_term::Style;
use rustyline::completion::{Completer, Pair};
use rustyline::config::{ColorMode, CompletionType, Config};
use rustyline::error::ReadlineError as RustyReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{
    Cmd, Context as RustylineContext, Editor, EventHandler, Helper, KeyCode, KeyEvent, Modifiers,
    Prompt as RustylinePrompt,
};

use super::completer::InputCompleter;
use super::zsh::paste::wrap_pasted_text;
use crate::highlighter::ForgeHighlighter;
use crate::model::ForgeCommandManager;
use crate::prompt::ForgePrompt;

// Default threshold for collapse (mirror `CollapseConfig::default()`).
// const PASTE_COLLAPSE_THRESHOLD: usize = 4096;

const HISTORY_CAPACITY: usize = 1024 * 1024;

/// Interactive terminal editor used by the Forge prompt.
pub struct ForgeEditor {
    editor: Editor<ForgeHelper, DefaultHistory>,
    history_file: PathBuf,
    pending_buffer: Option<String>,
}

/// Result of reading one prompt interaction from the terminal.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadResult {
    Success(String),
    Empty,
    Continue,
    Exit,
}

impl ForgeEditor {
    /// Creates a new interactive editor with history, completion, and
    /// highlighting.
    pub fn new(
        env: Environment,
        custom_history_path: Option<PathBuf>,
        manager: Arc<ForgeCommandManager>,
    ) -> Self {
        let history_file = env.history_path(custom_history_path.as_ref());
        let helper = ForgeHelper::new(env.cwd, manager);
        let config = Config::builder()
            .max_history_size(HISTORY_CAPACITY)
            .expect("rustyline history capacity should be valid")
            .completion_type(CompletionType::List)
            .completion_show_all_if_ambiguous(true)
            .color_mode(ColorMode::Forced)
            .enable_signals(true)
            .build();
        let mut editor = Editor::<ForgeHelper, DefaultHistory>::with_config(config)
            .expect("rustyline editor should initialize for an interactive terminal");

        // Best-effort enable bracketed paste mode (CSI ?2004h) on stderr. This is harmless
        // if the terminal doesn't support it - older terminals just drop the escape.
        if let Ok(seq) = std::str::from_utf8(b"\x1b[?2004h") {
            let _ = std::io::Write::write_all(&mut std::io::stderr(), seq.as_bytes());
        }

        editor.bind_sequence(
            KeyEvent(KeyCode::Enter, Modifiers::ALT),
            EventHandler::Simple(Cmd::Newline),
        );
        editor.bind_sequence(
            KeyEvent(KeyCode::Char('k'), Modifiers::CTRL),
            EventHandler::Simple(Cmd::ClearScreen),
        );
        editor.bind_sequence(
            KeyEvent(KeyCode::Char('K'), Modifiers::CTRL),
            EventHandler::Simple(Cmd::ClearScreen),
        );
        editor.set_helper(Some(helper));
        let _ = editor.load_history(&history_file);
        Self { editor, history_file, pending_buffer: None }
    }

    /// Returns a bounded channel that prints while preserving readline's buffer.
    pub fn external_output(&mut self) -> Option<std::sync::mpsc::SyncSender<String>> {
        use rustyline::ExternalPrinter;
        let mut printer = self.editor.create_external_printer().ok()?;
        let (sender, receiver) = std::sync::mpsc::sync_channel::<String>(256);
        std::thread::spawn(move || {
            while let Ok(line) = receiver.recv() {
                if printer.print(line).is_err() {
                    break;
                }
            }
        });
        Some(sender)
    }

    fn normalize_result(&mut self, buffer: String) -> ReadResult {
        let result = normalize_result_text(buffer);
        if let ReadResult::Success(text) = &result {
            let _ = self.editor.add_history_entry(text.as_str());
            let _ = self.editor.save_history(&self.history_file);
        }
        result
    }

    /// Reads one logical input from the terminal.
    pub fn prompt(&mut self, prompt: &mut ForgePrompt) -> anyhow::Result<ReadResult> {
        let prompt_text = render_prompt(prompt);
        let initial = self.pending_buffer.take().unwrap_or_default();
        let readline = if initial.is_empty() {
            self.editor.readline(&prompt_text)
        } else {
            self.editor
                .readline_with_initial(&prompt_text, (&initial, ""))
        };
        prompt.refresh();

        match readline {
            Ok(buffer) => Ok(self.normalize_result(buffer)),
            Err(RustyReadlineError::Interrupted) => Ok(ReadResult::Continue),
            Err(RustyReadlineError::Eof) => Ok(ReadResult::Exit),
            Err(error) => Err(anyhow::anyhow!(ReadLineError(error))),
        }
    }

    /// Sets the buffer content to be pre-filled on the next prompt.
    pub fn set_buffer(&mut self, content: String) {
        self.pending_buffer = Some(content);
    }
}

#[derive(Debug, thiserror::Error)]
#[error("failed to read line from terminal: {0}")]
pub struct ReadLineError(RustyReadlineError);

/// Threshold (bytes) above which we collapse a paste into a placeholder.
/// Mirrors `forge_paste::collapse::CollapseConfig::default().threshold_bytes`.
const PASTE_COLLAPSE_THRESHOLD: usize = 4096;

fn normalize_result_text(buffer: String) -> ReadResult {
    // Strip bracketed-paste sentinels that the terminal emitted (CSI ?2004h/l).
    let (stripped, _was_bracketed) =
        forge_paste::shell::strip_bracketed_sentinels(buffer.as_bytes());
    let stripped = String::from_utf8(stripped).unwrap_or_default();
    let trimmed = stripped.trim();
    if trimmed.is_empty() {
        return ReadResult::Empty;
    }

    // 1) Mentions first: rewrite @path / @dir / @sym / @agent / @git / @web to @[path] placeholders.
    let mentions = forge_paste::mention::parse(trimmed);
    let with_mentions = if !mentions.mentions.is_empty() {
        let mut out = trimmed.to_string();
        for m in &mentions.mentions {
            // Replace the raw mention text with the @[kind:payload] placeholder form.
            out = out.replace(&m.raw, &m.placeholder());
        }
        out
    } else {
        trimmed.to_string()
    };
    // 2) Classify: detect paste-like content (text vs code vs binary vs large blob).
    let classified =
        forge_paste::classifier::classify(with_mentions.as_bytes(), PASTE_COLLAPSE_THRESHOLD);
    let use_collapse = matches!(
        classified.kind,
        forge_paste::classifier::PasteKind::LargeBlob | forge_paste::classifier::PasteKind::Code
    ) || with_mentions.len() > PASTE_COLLAPSE_THRESHOLD;

    if use_collapse {
        let event =
            forge_paste::paste_event::PasteEvent::new_programmatic(with_mentions.into_bytes());
        let cfg = forge_paste::collapse::CollapseConfig::default();
        let outcome = forge_paste::collapse::collapse_paste(&event, &cfg);
        return ReadResult::Success(outcome.rewritten());
    }

    // 3) Fallback: legacy collapse-to-quoted-paths for any remaining bare path mentions.
    ReadResult::Success(wrap_pasted_text(&with_mentions))
}

fn render_prompt(prompt: &ForgePrompt) -> ResponsivePrompt {
    let left = prompt.render_prompt_left();
    let indicator = prompt.render_prompt_indicator();
    let right = prompt.render_prompt_right();
    let right = right.trim_start();

    // `raw` is what rustyline measures to position the cursor; `styled` is what
    // it prints. `raw` MUST be free of ANSI escapes: rustyline's Windows
    // console backend computes cursor columns by counting grapheme widths of
    // `raw` (it cannot interpret escape sequences and debug-asserts against
    // them), so any styling left in `raw` is counted as visible width and
    // pushes the cursor past where the text actually is. The left prompt and
    // indicator are styled via `nu_ansi_term`, so strip those codes for `raw`.
    // The right prompt is positioned off to the side with cursor save/restore
    // and is not part of the input-line geometry, so it is excluded from `raw`
    // entirely.
    if right.trim().is_empty() {
        let styled = format!("{left}{indicator}");
        let raw = strip_ansi_codes(&styled).into_owned();
        return ResponsivePrompt { raw, styled };
    }

    if let Some((first_line, remaining)) = left.split_once('\n') {
        let right = render_right_prompt(right);
        let raw = strip_ansi_codes(&format!("{first_line}\n{remaining}{indicator}")).into_owned();
        return ResponsivePrompt {
            raw,
            styled: format!("{first_line}{right}\n{remaining}{indicator}"),
        };
    }

    let right = render_right_prompt(right);
    let raw = strip_ansi_codes(&format!("{left}{indicator}")).into_owned();
    ResponsivePrompt { raw, styled: format!("{left}{right}{indicator}") }
}

fn render_right_prompt(right: &str) -> String {
    let width = measure_text_width(strip_ansi_codes(right).as_ref());
    format!("\x1b[s\x1b[999C\x1b[{width}D{right}\x1b[K\x1b[u")
}

struct ResponsivePrompt {
    raw: String,
    styled: String,
}

impl RustylinePrompt for ResponsivePrompt {
    fn raw(&self) -> &str {
        &self.raw
    }

    fn styled(&self) -> &str {
        &self.styled
    }
}

struct ForgeHelper {
    completer: Mutex<InputCompleter>,
    highlighter: ForgeHighlighter,
    hinter: HistoryHinter,
}

impl ForgeHelper {
    fn new(cwd: PathBuf, command_manager: Arc<ForgeCommandManager>) -> Self {
        Self {
            completer: Mutex::new(InputCompleter::new(cwd, command_manager)),
            highlighter: ForgeHighlighter,
            hinter: HistoryHinter {},
        }
    }
}

impl Helper for ForgeHelper {}

impl Completer for ForgeHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &RustylineContext<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let mut completer = self
            .completer
            .lock()
            .expect("input completer mutex poisoned");
        let suggestions = completer.complete(line, pos);
        let start = suggestions
            .iter()
            .map(|suggestion| suggestion.span.start)
            .min()
            .unwrap_or(pos);
        let pairs = suggestions
            .into_iter()
            .map(|suggestion| {
                let replacement = if suggestion.append_whitespace {
                    format!("{} ", suggestion.value)
                } else {
                    suggestion.value
                };
                Pair { display: replacement.clone(), replacement }
            })
            .collect();
        Ok((start, pairs))
    }
}

impl Hinter for ForgeHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &RustylineContext<'_>) -> Option<Self::Hint> {
        self.hinter.hint(line, pos, ctx)
    }
}

impl Highlighter for ForgeHelper {
    fn highlight<'l>(&self, line: &'l str, pos: usize) -> Cow<'l, str> {
        let styled = self.highlighter.highlight(line, pos);
        if styled.buffer.is_empty() {
            return Cow::Borrowed(line);
        }

        let default_style = Style::new();
        let mut rendered = String::with_capacity(line.len());
        for (style, text) in styled.buffer {
            if style == default_style {
                rendered.push_str(&text);
            } else {
                rendered.push_str(&style.paint(text).to_string());
            }
        }
        Cow::Owned(rendered)
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Owned(Style::new().dimmed().paint(hint).to_string())
    }
}

impl Validator for ForgeHelper {}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_normalize_result_wraps_existing_pasted_path() {
        // A real file on the current platform: unix paths like /usr/bin/env
        // do not exist on Windows, so create a fixture file instead.
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("existing.txt");
        std::fs::write(&file_path, "test").unwrap();
        let path = file_path.to_string_lossy().into_owned();

        let expected = ReadResult::Success(format!("@[{path}]"));
        let actual = normalize_result_text(path);

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_render_prompt_raw_has_no_ansi_escapes() {
        use std::path::PathBuf;

        use forge_api::{AgentId, ModelId};

        // rustyline measures `raw()` to position the cursor and, on Windows,
        // cannot interpret ANSI escapes (it counts their bytes as visible
        // columns). `raw()` must therefore be free of escape sequences even
        // though the visible prompt is styled.
        let mut prompt = ForgePrompt::new(PathBuf::from("project"), AgentId::default());
        prompt.model(ModelId::new("anthropic/claude-opus-4"));

        let rendered = render_prompt(&prompt);

        assert!(
            !rendered.raw.contains('\u{1b}'),
            "raw prompt must not contain ANSI escape sequences: {:?}",
            rendered.raw
        );
        // The styled prompt, by contrast, does carry styling for display.
        assert!(rendered.styled.contains('\u{1b}'));
    }
}
