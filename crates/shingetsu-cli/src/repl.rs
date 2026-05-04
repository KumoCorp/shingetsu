use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use termwiz::cell::{AttributeChange, Intensity};
use termwiz::color::ColorAttribute;
use termwiz::input::{InputEvent, KeyCode, KeyEvent, Modifiers};
use termwiz::lineedit::{
    Action, BasicHistory, CompletionCandidate, History, LineEditor, LineEditorHost, OutputElement,
};
use termwiz::surface::change::Change;
use termwiz::terminal::Terminal as _;

use shingetsu::GlobalEnv;
use shingetsu_repl::{completions, parse_status, ParseStatus, Repl, SubmitOutcome};

use crate::highlight::{highlight_lua, HighlightTheme};

// ---------------------------------------------------------------------------
// LuaHost
// ---------------------------------------------------------------------------

pub struct LuaHost {
    history: BasicHistory,
    env: GlobalEnv,
    /// Accumulated incomplete lines from prior `submit_line` calls.
    pending: String,
    theme: HighlightTheme,
}

impl LuaHost {
    pub fn new(env: GlobalEnv, theme: HighlightTheme) -> Self {
        Self {
            history: BasicHistory::default(),
            env,
            pending: String::new(),
            theme,
        }
    }
}

impl LineEditorHost for LuaHost {
    fn history(&mut self) -> &mut dyn History {
        &mut self.history
    }

    fn highlight_line(&self, line: &str, cursor_position: usize) -> (Vec<OutputElement>, usize) {
        let changes = highlight_lua(line, &self.theme);
        let elements = changes_to_output_elements(changes);
        let cursor_x = termwiz::cell::unicode_column_width(&line[..cursor_position], None);
        (elements, cursor_x)
    }

    fn render_preview(&self, line: &str) -> Vec<OutputElement> {
        let text = match parse_status(&self.pending, line) {
            ParseStatus::Complete => return vec![],
            ParseStatus::Incomplete => "...".to_string(),
            ParseStatus::Error(msg) => msg,
        };
        let mut out = Vec::new();
        out.push(OutputElement::Attribute(AttributeChange::Foreground(
            ColorAttribute::PaletteIndex(8),
        )));
        out.push(OutputElement::Attribute(AttributeChange::Intensity(
            Intensity::Half,
        )));
        // In raw terminal mode \n doesn't imply CR; emit each line with \r\n.
        for line in text.lines() {
            out.push(OutputElement::Text(format!("{line}\r\n")));
        }
        if !text.ends_with('\n') {
            out.push(OutputElement::Text("\r\n".to_string()));
        }
        out.push(OutputElement::Attribute(AttributeChange::Foreground(
            ColorAttribute::Default,
        )));
        out.push(OutputElement::Attribute(AttributeChange::Intensity(
            Intensity::Normal,
        )));
        out
    }

    fn resolve_action(&mut self, event: &InputEvent, _editor: &mut LineEditor) -> Option<Action> {
        // Map Ctrl-D to Cancel so it returns Ok(None) like Ctrl-C, avoiding
        // the Err(UnexpectedEof) that Action::EndOfFile would produce.
        match event {
            InputEvent::Key(KeyEvent {
                key:
                    KeyCode::Char('D') | KeyCode::Char('d') | KeyCode::Char('C') | KeyCode::Char('c'),
                modifiers: Modifiers::CTRL,
            }) => Some(Action::Cancel),
            _ => None,
        }
    }

    fn complete(&self, line: &str, cursor_position: usize) -> Vec<CompletionCandidate> {
        let (range, candidates) = completions(&self.env, &self.pending, line, cursor_position);
        candidates
            .into_iter()
            .map(|text| CompletionCandidate {
                range: range.clone(),
                text,
            })
            .collect()
    }
}

fn changes_to_output_elements(changes: Vec<Change>) -> Vec<OutputElement> {
    changes
        .into_iter()
        .map(|c| match c {
            Change::Attribute(a) => OutputElement::Attribute(a),
            Change::AllAttributes(a) => OutputElement::AllAttributes(a),
            Change::Text(t) => OutputElement::Text(t),
            // Ignore surface-level changes that don't map to OutputElement.
            _ => OutputElement::Text(String::new()),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// History persistence
// ---------------------------------------------------------------------------

fn history_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".local/share/shingetsu/history"))
}

pub fn load_history(host: &mut LuaHost) {
    let Some(path) = history_path() else { return };
    let Ok(file) = std::fs::File::open(&path) else {
        return;
    };
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        host.history.add(&line);
    }
}

pub fn save_history(host: &LuaHost) {
    let Some(path) = history_path() else { return };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let Ok(mut file) = std::fs::File::create(&path) else {
        return;
    };
    let count = host.history.last().map(|l| l + 1).unwrap_or(0);
    for idx in 0..count {
        if let Some(line) = host.history.get(idx) {
            writeln!(file, "{line}").ok();
        }
    }
}

// ---------------------------------------------------------------------------
// run_repl
// ---------------------------------------------------------------------------

pub async fn run_repl(env: GlobalEnv, theme: HighlightTheme) -> anyhow::Result<()> {
    let mut terminal = termwiz::lineedit::line_editor_terminal()?;
    let mut host = LuaHost::new(env.clone(), theme);
    load_history(&mut host);

    let mut repl = Repl::new(env);
    let mut exit_code: Option<i32> = None;

    loop {
        let prompt = repl.prompt().to_string();
        host.pending = repl.pending_lines().to_string();

        let line = tokio::task::block_in_place(|| {
            let mut editor = termwiz::lineedit::LineEditor::new(&mut terminal);
            editor.set_prompt(&prompt);
            editor.read_line(&mut host)
        })?;

        // Ctrl-C and Ctrl-D both return Ok(None) (Ctrl-D is remapped in resolve_action).
        let Some(line) = line else { break };

        // Add non-empty lines to history.
        if !line.trim().is_empty() {
            host.history.add(&line);
        }

        match repl.submit_line(&line).await {
            SubmitOutcome::Incomplete => {
                // Prompt will switch to ">> " on next iteration.
            }
            SubmitOutcome::Values(values) => {
                for v in &values {
                    let mut changes = highlight_lua(v, &host.theme);
                    changes.push(Change::Text("\r\n".to_string()));
                    terminal.render(&changes)?;
                }
                terminal.flush()?;
                shingetsu::flush_stdio().await;
            }
            SubmitOutcome::Error(msg) => {
                eprintln!("{msg}");
                shingetsu::flush_stdio().await;
            }
            SubmitOutcome::Exit { code, close } => {
                if close {
                    repl.env().dispose().await;
                }
                exit_code = Some(code);
                break;
            }
        }
    }

    save_history(&host);
    // Explicitly drop the terminal so UnixTerminal::Drop restores termios,
    // cursor, bracketed paste, etc. before any process::exit call.
    drop(terminal);
    shingetsu::flush_stdio().await;
    if let Some(code) = exit_code {
        std::process::exit(code);
    }
    Ok(())
}
