use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use forge_api::Environment;

use crate::editor::{ForgeEditor, ReadResult};
use crate::model::{AppCommand, ForgeCommandManager};
use crate::prompt::ForgePrompt;
use crate::tracker;

/// Console implementation for handling user input via command line.
pub struct Console {
    command: Arc<ForgeCommandManager>,
    output: Option<std::sync::mpsc::SyncSender<String>>,
    sessions:
        Mutex<std::collections::HashMap<forge_domain::ConversationId, crate::live_control::Handle>>,
    editor: Mutex<ForgeEditor>,
}

impl Console {
    /// Creates a new instance of `Console`.
    pub fn new(
        env: Environment,
        custom_history_path: Option<PathBuf>,
        command: Arc<ForgeCommandManager>,
    ) -> Self {
        let mut editor = ForgeEditor::new(env, custom_history_path, command.clone());
        let output = editor.external_output();
        Self {
            command,
            editor: Mutex::new(editor),
            output,
            sessions: Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl Console {
    pub fn register_live(&self, handle: &crate::live_control::Handle) {
        handle.set_output(self.output.clone());
        self.sessions
            .lock()
            .unwrap()
            .insert(handle.session_id, handle.clone());
    }

    pub async fn prompt(&self, prompt: &mut ForgePrompt) -> anyhow::Result<AppCommand> {
        loop {
            let mut forge_editor = self.editor.lock().unwrap();
            let user_input = forge_editor.prompt(prompt)?;

            drop(forge_editor);
            match user_input {
                ReadResult::Continue => continue,
                ReadResult::Exit => return Ok(AppCommand::Exit),
                ReadResult::Empty => continue,
                ReadResult::Success(text) => {
                    let sessions = self.sessions.lock().unwrap().clone();
                    match crate::live_control::terminal::command(&text, &sessions) {
                        Ok(true) => continue,
                        Err(error) => {
                            eprintln!("{error}");
                            continue;
                        }
                        Ok(false) => {}
                    }
                    tracker::prompt(text.clone());
                    return self.command.parse(&text);
                }
            }
        }
    }

    /// Sets the buffer content for the next prompt
    pub fn set_buffer(&self, content: String) {
        let mut editor = self.editor.lock().unwrap();
        editor.set_buffer(content);
    }

    /// Clear the terminal screen (ANSI escape sequence).
    /// Works in any TTY without needing a ConsoleWriter abstraction.
    pub fn clear_screen(&self) -> anyhow::Result<()> {
        use std::io::Write;
        // ANSI: clear entire screen, move cursor home
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(b"\x1b[2J\x1b[H")?;
        stdout.flush()?;
        Ok(())
    }
}
