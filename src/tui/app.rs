use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::task::JoinHandle;

use crate::indexer::run_index;
use crate::pack::{init_pack, load_file_state, load_index, load_manifest};
use crate::query::run_query;
use crate::tui::ui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Init,
    Index,
    Query,
    Status,
    Serve,
}

impl Screen {
    fn next(self) -> Self {
        match self {
            Self::Init => Self::Index,
            Self::Index => Self::Query,
            Self::Query => Self::Status,
            Self::Status => Self::Serve,
            Self::Serve => Self::Init,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Init => Self::Serve,
            Self::Index => Self::Init,
            Self::Query => Self::Index,
            Self::Status => Self::Query,
            Self::Serve => Self::Status,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Init => "Init",
            Self::Index => "Index",
            Self::Query => "Query",
            Self::Status => "Status",
            Self::Serve => "Serve",
        }
    }
}

pub struct App {
    pub screen: Screen,
    pub field_idx: usize,
    pub pack: String,
    pub init_provider: String,
    pub init_model: String,
    pub init_dim: String,
    pub init_force: bool,
    pub index_sources: String,
    pub query_text: String,
    pub query_mode: String,
    pub query_top_k: String,
    pub serve_host: String,
    pub serve_port: String,
    pub status_lines: Vec<String>,
    pub output_lines: Vec<String>,
    pub server_running: bool,
    server_task: Option<JoinHandle<()>>,
    server_events: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            screen: Screen::Init,
            field_idx: 0,
            pack: "./memory-pack".to_string(),
            init_provider: "fastembed".to_string(),
            init_model: "BAAI/bge-small-en-v1.5".to_string(),
            init_dim: "384".to_string(),
            init_force: false,
            index_sources: String::new(),
            query_text: String::new(),
            query_mode: "hybrid".to_string(),
            query_top_k: "8".to_string(),
            serve_host: "127.0.0.1".to_string(),
            serve_port: "7821".to_string(),
            status_lines: Vec::new(),
            output_lines: vec!["Press q to quit, Left/Right to change screen.".to_string()],
            server_running: false,
            server_task: None,
            server_events: None,
        }
    }
}

impl App {
    fn field_count(&self) -> usize {
        match self.screen {
            Screen::Init => 5,
            Screen::Index => 2,
            Screen::Query => 4,
            Screen::Status => 1,
            Screen::Serve => 3,
        }
    }

    fn push_output(&mut self, line: impl Into<String>) {
        self.output_lines.push(line.into());
        if self.output_lines.len() > 12 {
            let keep_from = self.output_lines.len().saturating_sub(12);
            self.output_lines.drain(0..keep_from);
        }
    }

    fn selected_text_mut(&mut self) -> Option<&mut String> {
        match self.screen {
            Screen::Init => match self.field_idx {
                0 => Some(&mut self.pack),
                1 => Some(&mut self.init_provider),
                2 => Some(&mut self.init_model),
                3 => Some(&mut self.init_dim),
                _ => None,
            },
            Screen::Index => match self.field_idx {
                0 => Some(&mut self.pack),
                1 => Some(&mut self.index_sources),
                _ => None,
            },
            Screen::Query => match self.field_idx {
                0 => Some(&mut self.pack),
                1 => Some(&mut self.query_text),
                2 => Some(&mut self.query_mode),
                3 => Some(&mut self.query_top_k),
                _ => None,
            },
            Screen::Status => match self.field_idx {
                0 => Some(&mut self.pack),
                _ => None,
            },
            Screen::Serve => match self.field_idx {
                0 => Some(&mut self.pack),
                1 => Some(&mut self.serve_host),
                2 => Some(&mut self.serve_port),
                _ => None,
            },
        }
    }

    fn execute_action(&mut self) -> Result<()> {
        match self.screen {
            Screen::Init => {
                let dim = self
                    .init_dim
                    .parse::<usize>()
                    .map_err(|_| anyhow!("dim must be a number"))?;
                let pack = PathBuf::from(self.pack.trim());
                init_pack(
                    &pack,
                    self.init_force,
                    self.init_provider.trim(),
                    self.init_model.trim(),
                    dim,
                )?;
                self.push_output(format!("initialized pack {}", pack.display()));
            }
            Screen::Index => {
                let pack = PathBuf::from(self.pack.trim());
                let sources: Vec<PathBuf> = self
                    .index_sources
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect();
                if sources.is_empty() {
                    return Err(anyhow!("index requires at least one source path"));
                }
                let (scanned, updated, chunks) = run_index(&pack, &sources)?;
                self.push_output(format!(
                    "index complete: scanned={} updated_files={} chunks={}",
                    scanned, updated, chunks
                ));
            }
            Screen::Query => {
                let pack = PathBuf::from(self.pack.trim());
                let top_k = self
                    .query_top_k
                    .parse::<usize>()
                    .map_err(|_| anyhow!("top_k must be a number"))?;
                let response =
                    run_query(&pack, self.query_text.trim(), self.query_mode.trim(), top_k)?;
                self.push_output(format!("query mode={}", response.mode));
                if response.results.is_empty() {
                    self.push_output("no results".to_string());
                } else {
                    for hit in response.results.iter().take(5) {
                        self.push_output(format!(
                            "[{:.3}] {} ({})",
                            hit.score, hit.file_path, hit.chunk_id
                        ));
                    }
                }
            }
            Screen::Status => {
                let pack = PathBuf::from(self.pack.trim());
                let manifest = load_manifest(&pack)?;
                let index = load_index(&pack)?;
                let states = load_file_state(&pack)?;
                self.status_lines = vec![
                    format!("pack: {}", pack.display()),
                    format!("pack_id: {}", manifest.pack_id),
                    format!("format: {}", manifest.format_version),
                    format!(
                        "embedding: provider={} model={} dim={}",
                        manifest.embedding.provider,
                        manifest.embedding.model,
                        manifest.embedding.dimension
                    ),
                    format!("sources: {}", manifest.sources.len()),
                    format!("files_state: {}", states.len()),
                    format!("chunks: {}", index.docs.len()),
                ];
            }
            Screen::Serve => {
                if self.server_running {
                    if let Some(handle) = self.server_task.take() {
                        handle.abort();
                    }
                    self.server_events = None;
                    self.server_running = false;
                    self.push_output("server stopped".to_string());
                } else {
                    let pack = PathBuf::from(self.pack.trim());
                    let host = self.serve_host.trim().to_string();
                    let port = self
                        .serve_port
                        .trim()
                        .parse::<u16>()
                        .map_err(|_| anyhow!("port must be a number"))?;
                    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                    let handle = tokio::spawn(async move {
                        if let Err(err) = crate::serve_with_startup(pack, host, port).await {
                            let _ = tx.send(format!("server failed: {}", err));
                        } else {
                            let _ = tx.send("server exited".to_string());
                        }
                    });
                    self.server_task = Some(handle);
                    self.server_events = Some(rx);
                    self.server_running = true;
                    self.push_output("server started".to_string());
                }
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.kind != KeyEventKind::Press {
            return Ok(false);
        }

        if key.code == KeyCode::Char('q') && key.modifiers == KeyModifiers::NONE {
            if let Some(handle) = self.server_task.take() {
                handle.abort();
            }
            return Ok(true);
        }

        match key.code {
            KeyCode::Left => {
                self.screen = self.screen.prev();
                self.field_idx = 0;
            }
            KeyCode::Right => {
                self.screen = self.screen.next();
                self.field_idx = 0;
            }
            KeyCode::Tab => {
                self.field_idx = (self.field_idx + 1) % self.field_count();
            }
            KeyCode::BackTab => {
                let fields = self.field_count();
                self.field_idx = (self.field_idx + fields - 1) % fields;
            }
            KeyCode::Enter => {
                if let Err(err) = self.execute_action() {
                    self.push_output(format!("error: {}", err));
                }
            }
            KeyCode::Backspace => {
                if let Some(s) = self.selected_text_mut() {
                    s.pop();
                }
            }
            KeyCode::Char(' ') => {
                if self.screen == Screen::Init && self.field_idx == 4 {
                    self.init_force = !self.init_force;
                } else if let Some(s) = self.selected_text_mut() {
                    s.push(' ');
                }
            }
            KeyCode::Char(c) => {
                if let Some(s) = self.selected_text_mut() {
                    s.push(c);
                }
            }
            _ => {}
        }

        Ok(false)
    }

    async fn pump_server_events(&mut self) {
        let mut messages = Vec::new();
        if let Some(rx) = self.server_events.as_mut() {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        for msg in messages {
            self.push_output(msg);
            self.server_running = false;
            self.server_task = None;
        }
    }
}

pub async fn run_tui() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::default();
    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        app.pump_server_events().await;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if app.handle_key(key)? {
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
