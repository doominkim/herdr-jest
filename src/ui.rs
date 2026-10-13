use crate::args::Scope;
use crate::discover::NodeKind;
use crate::paths;
use crate::protocol::{FileView, NodeView, Request, RunRecord, Snapshot};
use crate::results::{Assertion, Status};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, HighlightSpacing, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Row {
    depth: usize,
    key: String,
    label: String,
    status: Status,
    running: bool,
    kind: RowKind,
    file: PathBuf,
    full_name: String,
    node_path: Vec<usize>,
    dynamic: bool,
    has_children: bool,
    counts: (usize, usize, usize),
    duration_ms: Option<u64>,
    indent: usize,
    show_dir: bool,
    files: Vec<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Folder,
    File,
    Describe,
    Test,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tests,
    History,
    Details,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ViewMode {
    #[default]
    Tree,
    Folders,
    Files,
}

impl ViewMode {
    fn next(self) -> ViewMode {
        match self {
            ViewMode::Tree => ViewMode::Folders,
            ViewMode::Folders => ViewMode::Files,
            ViewMode::Files => ViewMode::Tree,
        }
    }

    fn name(self) -> &'static str {
        match self {
            ViewMode::Tree => "tree",
            ViewMode::Folders => "folders",
            ViewMode::Files => "files",
        }
    }
}

#[derive(Default)]
struct DirNode<'a> {
    dirs: std::collections::BTreeMap<String, DirNode<'a>>,
    files: Vec<&'a FileView>,
}

#[derive(Default, Serialize, Deserialize)]
struct SavedView {
    expanded: BTreeSet<String>,
    #[serde(default)]
    collapsed_folders: BTreeSet<String>,
    #[serde(default)]
    mode: ViewMode,
    selected: usize,
}

struct App {
    root: PathBuf,
    snapshot: Snapshot,
    connected: bool,
    expanded: BTreeSet<String>,
    list: ListState,
    failed_only: bool,
    query: String,
    searching: bool,
    detail_scroll: u16,
    stream: Option<UnixStream>,
    started: Instant,
    focus: Focus,
    history_list: ListState,
    mode: ViewMode,
    collapsed_folders: BTreeSet<String>,
    targets: Vec<ClickTarget>,
    tree_area: Rect,
    history_area: Rect,
    detail_area: Rect,
    last_click: Option<(Instant, Focus, usize)>,
}

#[derive(Clone, Copy)]
enum ClickAction {
    Url(&'static str),
    Key(KeyCode),
}

struct ClickTarget {
    y: u16,
    start: u16,
    end: u16,
    action: ClickAction,
}

pub fn run(cwd: &Path) -> std::io::Result<()> {
    let Some(root) = paths::find_root(cwd) else {
        eprintln!("herdr-jest: no Jest config found in {} or its parents", cwd.display());
        wait_for_key();
        return Ok(());
    };
    let root = root.canonicalize()?;
    let snapshot = std::fs::read(paths::state_file(&root))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Snapshot>(&bytes).ok())
        .unwrap_or_else(|| Snapshot { root: root.clone(), ..Snapshot::default() });

    let stream = connect_or_spawn(&root);
    let (tx, rx) = mpsc::channel();
    if let Some(reader) = stream.as_ref().and_then(|stream| stream.try_clone().ok()) {
        thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                if let Ok(snapshot) = serde_json::from_str::<Snapshot>(&line)
                    && tx.send(snapshot).is_err()
                {
                    return;
                }
            }
        });
    }

    let view_file = paths::view_file(&root);
    let saved_view: SavedView = std::fs::read(&view_file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();

    let mut app = App {
        root,
        connected: stream.is_some(),
        snapshot,
        expanded: saved_view.expanded,
        list: ListState::default().with_selected(Some(saved_view.selected)),
        failed_only: false,
        query: String::new(),
        searching: false,
        detail_scroll: 0,
        stream,
        started: Instant::now(),
        focus: Focus::Tests,
        history_list: ListState::default().with_selected(Some(0)),
        mode: saved_view.mode,
        collapsed_folders: saved_view.collapsed_folders,
        targets: Vec::new(),
        tree_area: Rect::default(),
        history_area: Rect::default(),
        detail_area: Rect::default(),
        last_click: None,
    };
    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    let result = app.event_loop(&mut terminal, rx);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();

    let view = SavedView {
        expanded: app.expanded,
        collapsed_folders: app.collapsed_folders,
        mode: app.mode,
        selected: app.list.selected().unwrap_or(0),
    };
    if let Ok(json) = serde_json::to_vec(&view) {
        let _ = std::fs::write(view_file, json);
    }
    result
}

impl App {
    fn event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal, rx: Receiver<Snapshot>) -> std::io::Result<()> {
        loop {
            while let Ok(snapshot) = rx.try_recv() {
                let new_runs = snapshot.runs_completed.saturating_sub(self.snapshot.runs_completed) as usize;
                let run_finished = new_runs > 0;
                if let Some(selected) = self.history_list.selected().filter(|_| run_finished && !self.snapshot.history.is_empty()) {
                    self.history_list.select(Some(selected + new_runs));
                }
                self.snapshot = snapshot;
                if run_finished {
                    self.expand_failed();
                }
            }
            let rows = self.rows();
            let selected = self.list.selected().unwrap_or(0).min(rows.len().saturating_sub(1));
            self.list.select(Some(selected));
            terminal.draw(|frame| self.draw(frame, &rows))?;

            if !event::poll(Duration::from_millis(150))? {
                continue;
            }
            let key = match event::read()? {
                Event::Key(key) => key,
                Event::Mouse(mouse) => {
                    let keep_going = match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => self.on_click(mouse.column, mouse.row, &rows),
                        MouseEventKind::ScrollDown => self.on_scroll(mouse.column, mouse.row, 3, &rows),
                        MouseEventKind::ScrollUp => self.on_scroll(mouse.column, mouse.row, -3, &rows),
                        _ => true,
                    };
                    if !keep_going {
                        return Ok(());
                    }
                    continue;
                }
                _ => continue,
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if self.searching {
                self.on_search_key(key);
                continue;
            }
            if !self.on_key(key, &rows) {
                return Ok(());
            }
        }
    }

    fn on_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.searching = false,
            KeyCode::Esc => {
                self.searching = false;
                self.query.clear();
            }
            KeyCode::Backspace => {
                self.query.pop();
            }
            KeyCode::Char(c) => self.query.push(c),
            _ => {}
        }
        self.list.select(Some(0));
    }

    fn on_key(&mut self, key: KeyEvent, rows: &[Row]) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::F(2) => return false,
            KeyCode::Char('d') if ctrl => self.detail_scroll = self.detail_scroll.saturating_add(10),
            KeyCode::Char('u') if ctrl => self.detail_scroll = self.detail_scroll.saturating_sub(10),
            KeyCode::Char('1') => self.focus = Focus::Tests,
            KeyCode::Char('2') => self.set_focus(Focus::History),
            KeyCode::Char('3') => self.focus = Focus::Details,
            KeyCode::Tab => {
                let next = match self.focus {
                    Focus::Tests => Focus::History,
                    Focus::History => Focus::Details,
                    Focus::Details => Focus::Tests,
                };
                self.set_focus(next);
            }
            KeyCode::Char('a') => self.send(Request::Run { scope: Scope::All }),
            KeyCode::Char('f') => self.send(Request::RunFailed),
            KeyCode::Char('F') => {
                self.focus = Focus::Tests;
                self.failed_only = !self.failed_only;
                self.list.select(Some(0));
            }
            KeyCode::Char('v') => {
                self.focus = Focus::Tests;
                self.mode = self.mode.next();
                self.list.select(Some(0));
            }
            KeyCode::Char('/') => {
                self.focus = Focus::Tests;
                self.searching = true;
                self.query.clear();
            }
            KeyCode::Char('X') => {
                self.send(Request::Stop);
                return false;
            }
            _ => match self.focus {
                Focus::Tests => self.on_tests_key(key, rows),
                Focus::History => self.on_history_key(key),
                Focus::Details => self.on_details_key(key),
            },
        }
        true
    }

    fn set_focus(&mut self, focus: Focus) {
        if focus != self.focus {
            self.detail_scroll = 0;
        }
        self.focus = focus;
    }

    fn on_history_key(&mut self, key: KeyEvent) {
        let len = self.snapshot.history.len();
        let selected = self.history_list.selected().unwrap_or(0).min(len.saturating_sub(1));
        match key.code {
            KeyCode::Char('j') | KeyCode::Down if len > 0 => {
                self.history_list.select(Some((selected + 1).min(len - 1)));
                self.detail_scroll = 0;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.history_list.select(Some(selected.saturating_sub(1)));
                self.detail_scroll = 0;
            }
            KeyCode::Enter => {
                if let Some(scope) = self.snapshot.history.get(selected).and_then(|run| run.scope.clone()) {
                    self.send(Request::Run { scope });
                }
            }
            _ => {}
        }
    }

    fn on_details_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.detail_scroll = self.detail_scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => self.detail_scroll = self.detail_scroll.saturating_sub(1),
            _ => {}
        }
    }

    fn on_tests_key(&mut self, key: KeyEvent, rows: &[Row]) {
        let selected = self.list.selected().unwrap_or(0);
        let row = rows.get(selected);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(selected, 1, rows.len()),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(selected, -1, rows.len()),
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(row) = row.filter(|row| row.has_children) {
                    if self.is_open(row) {
                        self.move_selection(selected, 1, rows.len());
                    } else {
                        self.set_open(row, true);
                    }
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.collapse_or_parent(selected, rows),
            KeyCode::Enter => {
                if let Some(row) = row {
                    self.send(Request::Run { scope: scope_for(row) });
                }
            }
            _ => {}
        }
    }

    fn move_selection(&mut self, selected: usize, delta: isize, len: usize) {
        if len == 0 {
            return;
        }
        let next = (selected as isize + delta).clamp(0, len as isize - 1) as usize;
        self.list.select(Some(next));
        self.detail_scroll = 0;
    }

    fn is_open(&self, row: &Row) -> bool {
        match row.kind {
            RowKind::Folder => !self.collapsed_folders.contains(&row.key),
            _ => self.expanded.contains(&row.key),
        }
    }

    fn set_open(&mut self, row: &Row, open: bool) {
        let set = match row.kind {
            RowKind::Folder => &mut self.collapsed_folders,
            _ => &mut self.expanded,
        };
        let insert = if row.kind == RowKind::Folder { !open } else { open };
        if insert {
            set.insert(row.key.clone());
        } else {
            set.remove(&row.key);
        }
    }

    fn collapse_or_parent(&mut self, selected: usize, rows: &[Row]) {
        let Some(row) = rows.get(selected) else {
            return;
        };
        if row.has_children && self.is_open(row) {
            self.set_open(row, false);
            return;
        }
        let parent = rows[..selected].iter().rposition(|candidate| candidate.depth < row.depth);
        if let Some(parent) = parent {
            self.list.select(Some(parent));
        }
    }

    fn send(&mut self, request: Request) {
        let Some(stream) = self.stream.as_mut() else {
            return;
        };
        let Ok(mut line) = serde_json::to_vec(&request) else {
            return;
        };
        line.push(b'\n');
        if stream.write_all(&line).is_err() {
            self.stream = None;
            self.connected = false;
        }
    }

    fn expand_failed(&mut self) {
        for file in &self.snapshot.files {
            let file_key = file.path.display().to_string();
            if file.status == Status::Failed {
                self.expanded.insert(file_key.clone());
                expand_failed_nodes(&mut self.expanded, &file_key, &file.nodes);
            }
        }
    }

    fn rows(&self) -> Vec<Row> {
        match self.mode {
            ViewMode::Tree => self.snapshot.files.iter().filter_map(|file| self.file_rows(file, 0, true, true)).flatten().collect(),
            ViewMode::Files => self.snapshot.files.iter().filter_map(|file| self.file_rows(file, 0, true, false)).flatten().collect(),
            ViewMode::Folders => {
                let mut root = DirNode::default();
                for file in &self.snapshot.files {
                    let relative = file.path.strip_prefix(&self.root).unwrap_or(&file.path);
                    let mut dir = &mut root;
                    if let Some(parent) = relative.parent() {
                        for component in parent.components() {
                            let name = component.as_os_str().to_string_lossy().to_string();
                            dir = dir.dirs.entry(name).or_default();
                        }
                    }
                    dir.files.push(file);
                }
                let mut rows = Vec::new();
                self.folder_rows(&root, "", 0, &mut rows);
                rows
            }
        }
    }

    fn folder_rows(&self, dir: &DirNode, prefix: &str, depth: usize, out: &mut Vec<Row>) {
        let running: BTreeSet<&PathBuf> = self.snapshot.running_files.iter().collect();
        for (name, child) in &dir.dirs {
            let mut label = name.clone();
            let mut node = child;
            while node.files.is_empty() && node.dirs.len() == 1 {
                let (next_name, next) = node.dirs.iter().next().expect("one child");
                label = format!("{label}/{next_name}");
                node = next;
            }
            let path = if prefix.is_empty() { label.clone() } else { format!("{prefix}/{label}") };
            let mut children = Vec::new();
            self.folder_rows(node, &path, depth + 1, &mut children);
            if children.is_empty() {
                continue;
            }
            let files: Vec<PathBuf> = children
                .iter()
                .filter(|row| row.kind == RowKind::File)
                .map(|row| row.file.clone())
                .collect();
            let file_views: Vec<&FileView> =
                self.snapshot.files.iter().filter(|file| files.contains(&file.path)).collect();
            let status = folder_status(file_views.iter().map(|file| file.status));
            let counts = file_views
                .iter()
                .map(|file| counts(&file.nodes))
                .fold((0, 0, 0), |(p, f, s), (fp, ff, fs)| (p + fp, f + ff, s + fs));
            let row = Row {
                depth,
                key: format!("dir:{path}"),
                label,
                status,
                running: files.iter().any(|file| running.contains(file)),
                kind: RowKind::Folder,
                file: self.root.join(&path),
                full_name: path.clone(),
                node_path: Vec::new(),
                dynamic: false,
                has_children: true,
                counts,
                duration_ms: None,
                indent: depth,
                show_dir: false,
                files,
            };
            let open = self.is_open(&row) || !self.query.is_empty() || self.failed_only;
            out.push(row);
            if open {
                out.extend(children);
            }
        }
        for file in &dir.files {
            if let Some(rows) = self.file_rows(file, depth, false, true) {
                out.extend(rows);
            }
        }
    }

    fn file_rows(&self, file: &FileView, depth: usize, show_dir: bool, with_children: bool) -> Option<Vec<Row>> {
        if self.failed_only && file.status != Status::Failed {
            return None;
        }
        let query = self.query.to_lowercase();
        let relative = file.path.strip_prefix(&self.root).unwrap_or(&file.path).display().to_string();
        let file_matches = query.is_empty() || relative.to_lowercase().contains(&query);
        let key = file.path.display().to_string();
        let forced_open = !query.is_empty() && !file_matches;
        let open = with_children && (self.expanded.contains(&key) || forced_open);
        let mut children = Vec::new();
        let any_child = self.node_rows(file, &file.nodes, &key, &[], depth, open, file_matches, &mut children);
        if !file_matches && !any_child {
            return None;
        }
        let mut rows = vec![Row {
            depth,
            key,
            label: relative,
            status: file.status,
            running: self.snapshot.running_files.contains(&file.path),
            kind: RowKind::File,
            file: file.path.clone(),
            full_name: String::new(),
            node_path: Vec::new(),
            dynamic: false,
            has_children: with_children && !file.nodes.is_empty(),
            counts: counts(&file.nodes),
            duration_ms: None,
            indent: depth,
            show_dir,
            files: vec![file.path.clone()],
        }];
        rows.extend(children);
        Some(rows)
    }

    #[allow(clippy::too_many_arguments)]
    fn node_rows(
        &self,
        file: &FileView,
        nodes: &[NodeView],
        parent_key: &str,
        parent_path: &[usize],
        file_depth: usize,
        visible: bool,
        include_all: bool,
        out: &mut Vec<Row>,
    ) -> bool {
        let query = self.query.to_lowercase();
        let mut any = false;
        for (index, node) in nodes.iter().enumerate() {
            if self.failed_only && node.status != Status::Failed {
                continue;
            }
            let key = format!("{parent_key}/{index}");
            let mut node_path = parent_path.to_vec();
            node_path.push(index);
            let self_matches = include_all || node.name.to_lowercase().contains(&query);
            let open = self.expanded.contains(&key) || (!query.is_empty() && !self_matches);
            let mut children = Vec::new();
            let child_matches =
                self.node_rows(file, &node.children, &key, &node_path, file_depth, visible && open, self_matches, &mut children);
            if !self_matches && !child_matches {
                continue;
            }
            any = true;
            if visible {
                let durations: Vec<u64> = node.assertions.iter().filter_map(|a| a.duration_ms).collect();
                out.push(Row {
                    depth: file_depth + node_path.len(),
                    key,
                    label: node.name.clone(),
                    status: node.status,
                    running: false,
                    kind: match node.kind {
                        NodeKind::Describe => RowKind::Describe,
                        NodeKind::Test => RowKind::Test,
                    },
                    file: file.path.clone(),
                    full_name: node.full_name.clone(),
                    node_path,
                    dynamic: node.dynamic,
                    has_children: !node.children.is_empty(),
                    counts: counts(&node.children),
                    duration_ms: (!durations.is_empty()).then(|| durations.iter().sum()),
                    indent: file_depth,
                    show_dir: false,
                    files: Vec::new(),
                });
                out.extend(children);
            }
        }
        any
    }

    fn draw(&mut self, frame: &mut Frame, rows: &[Row]) {
        let error = self.snapshot.last_error.as_deref().and_then(|error| {
            error.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string)
        });
        let [header, body, error_bar, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(u16::from(error.is_some())),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [left_area, detail_area] =
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(body);
        let history_height = self.snapshot.history.len().clamp(1, HISTORY_ROWS) as u16 + 2;
        let [tree_area, history_area] =
            Layout::vertical([Constraint::Min(5), Constraint::Length(history_height)]).areas(left_area);
        self.tree_area = tree_area;
        self.history_area = history_area;
        self.detail_area = detail_area;

        frame.render_widget(Paragraph::new(self.header_line(header.width)), header);

        let inner_width = tree_area.width.saturating_sub(3) as usize;
        let selected_index = self.list.selected();
        let items: Vec<ListItem> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let line = self.row_line(row, inner_width);
                ListItem::new(if selected_index == Some(index) { selected_line(line) } else { line })
            })
            .collect();
        let mut title_spans = vec![
            Span::raw(" [1] Tests "),
            Span::styled(format!("· {} ", self.mode.name()), Style::default().fg(DIM)),
        ];
        if self.searching || !self.query.is_empty() {
            title_spans.push(Span::styled(format!("/{} ", self.query), Style::default().fg(ACCENT)));
        }
        if self.failed_only {
            title_spans.push(Span::styled("failed only ", Style::default().fg(Color::Red)));
        }
        let title = Line::from(title_spans);
        let list = List::new(items)
            .block(panel(title, self.focus == Focus::Tests))
            .highlight_symbol(Span::styled("▌", Style::default().fg(ACCENT)))
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_style(Style::default().add_modifier(Modifier::BOLD));
        frame.render_stateful_widget(list, tree_area, &mut self.list);

        let history_focused = self.focus == Focus::History;
        let history_block = panel(Line::raw(" [2] History "), history_focused);
        if self.snapshot.history.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::styled(" No runs yet", Style::default().fg(DIM))).block(history_block),
                history_area,
            );
        } else {
            let history_width = history_area.width.saturating_sub(3) as usize;
            let items: Vec<ListItem> =
                self.snapshot.history.iter().map(|run| ListItem::new(history_line(run, history_width))).collect();
            let selected = self.history_list.selected().unwrap_or(0).min(items.len() - 1);
            self.history_list.select(Some(selected));
            let highlight = if history_focused { Style::default().bg(SELECTED_BG) } else { Style::default() };
            let symbol = if history_focused { "▌" } else { " " };
            let list = List::new(items)
                .block(history_block)
                .highlight_symbol(Span::styled(symbol, Style::default().fg(ACCENT)))
                .highlight_spacing(HighlightSpacing::Always)
                .highlight_style(highlight);
            frame.render_stateful_widget(list, history_area, &mut self.history_list);
        }

        let selected = self.list.selected().and_then(|index| rows.get(index));
        let (detail_title, detail_text) = match self.selected_run() {
            Some(run) if history_focused => (format!("run · {}", local_time(run.finished_at)), run_detail(run)),
            _ => (self.detail_title(selected), self.detail_text(selected)),
        };
        let detail = Paragraph::new(detail_text)
            .block(panel(Line::raw(format!(" [3] {detail_title} ")), self.focus == Focus::Details))
            .wrap(Wrap { trim: false })
            .scroll((self.detail_scroll, 0));
        frame.render_widget(detail, detail_area);

        if let Some(error) = error {
            frame.render_widget(
                Paragraph::new(Line::styled(format!(" ⚠ {error}"), Style::default().fg(Color::Red))),
                error_bar,
            );
        }
        let (footer_line, targets) = self.footer_line(footer.width);
        self.targets = targets
            .into_iter()
            .map(|(start, end, action)| ClickTarget { y: footer.y, start: footer.x + start, end: footer.x + end, action })
            .collect();
        frame.render_widget(Paragraph::new(footer_line), footer);
    }

    fn header_line(&self, width: u16) -> Line<'static> {
        let repo = self.root.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let parent = self
            .root
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|name| format!("{}/", name.to_string_lossy()))
            .unwrap_or_default();
        let left = vec![
            Span::styled(" herdr-jest ", Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {parent}"), Style::default().fg(DIM)),
            Span::styled(repo, Style::default().add_modifier(Modifier::BOLD)),
        ];
        let (passed, failed, skipped) = self
            .snapshot
            .files
            .iter()
            .map(|file| counts(&file.nodes))
            .fold((0, 0, 0), |(p, f, s), (fp, ff, fs)| (p + fp, f + ff, s + fs));
        let mut right = vec![
            Span::styled(format!("✓ {passed} passed"), Style::default().fg(Color::Green)),
            Span::raw("  "),
            Span::styled(
                format!("✗ {failed} failed"),
                if failed > 0 { Style::default().fg(Color::Red).add_modifier(Modifier::BOLD) } else { Style::default().fg(DIM) },
            ),
            Span::raw("  "),
            Span::styled(format!("○ {skipped} skipped"), Style::default().fg(DIM)),
        ];
        if let Some(ms) = self.snapshot.last_duration_ms {
            right.push(Span::styled(format!("  {}", format_duration(ms)), Style::default().fg(DIM)));
        }
        right.push(Span::raw(" "));
        justify(left, right, width as usize)
    }

    fn row_line(&self, row: &Row, width: usize) -> Line<'static> {
        let open = self.is_open(row) || (row.kind == RowKind::Folder && (!self.query.is_empty() || self.failed_only));
        let fold = match (row.has_children, open) {
            (false, _) => "  ",
            (true, true) => "▾ ",
            (true, false) => "▸ ",
        };
        let mut left: Vec<Span<'static>> = Vec::new();
        let mut right: Vec<Span<'static>> = Vec::new();
        left.push(Span::raw("  ".repeat(row.indent)));
        match row.kind {
            RowKind::Folder => {
                left.push(Span::styled(fold, Style::default().fg(DIM)));
                let (icon, color) = icon(row.status);
                left.push(Span::styled(format!("{icon} "), Style::default().fg(color)));
                right.extend(count_spans(row.counts));
                let used: usize = left.iter().chain(&right).map(|span| span.content.width()).sum();
                let label = ellipsize_middle(&format!("{}/", row.label), width.saturating_sub(used + 1));
                left.push(Span::styled(label, Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)));
            }
            RowKind::File => {
                left.push(Span::styled(fold, Style::default().fg(DIM)));
                left.push(self.badge(row));
                left.push(Span::raw(" "));
                let (dir, name) = match row.label.rsplit_once('/') {
                    Some((dir, name)) if row.show_dir => (dir.to_string(), name.to_string()),
                    Some((_, name)) => (String::new(), name.to_string()),
                    None => (String::new(), row.label.clone()),
                };
                right.extend(count_spans(row.counts));
                let used: usize = left.iter().chain(&right).map(|span| span.content.width()).sum();
                let available = width.saturating_sub(used + 2);
                let name = ellipsize_middle(&name, available);
                let dir_room = available.saturating_sub(name.width() + 3);
                left.push(Span::styled(name, Style::default().add_modifier(Modifier::BOLD)));
                if !dir.is_empty() && dir_room >= 4 {
                    left.push(Span::styled(format!("  {}", ellipsize_middle(&dir, dir_room)), Style::default().fg(DIM)));
                }
            }
            RowKind::Describe | RowKind::Test => {
                left.push(Span::raw("  "));
                let node_depth = row.node_path.len();
                if node_depth > 1 {
                    left.push(Span::styled("│ ".repeat(node_depth - 1), Style::default().fg(GUIDE)));
                }
                left.push(Span::styled(fold, Style::default().fg(DIM)));
                let (icon, color) = icon(row.status);
                left.push(Span::styled(format!("{icon} "), Style::default().fg(color)));
                let name_style = match (row.kind, row.status) {
                    (_, Status::Failed) => Style::default().fg(Color::Red),
                    (_, Status::Skipped) | (_, Status::Unknown) => Style::default().fg(DIM),
                    (RowKind::Describe, _) => Style::default().add_modifier(Modifier::BOLD),
                    _ => Style::default(),
                };
                left.push(Span::styled(row.label.clone(), name_style));
                if let Some(ms) = row.duration_ms {
                    right.push(Span::styled(format_duration(ms), Style::default().fg(DIM)));
                }
            }
        }
        right.push(Span::raw(" "));
        justify(left, right, width)
    }

    fn badge(&self, row: &Row) -> Span<'static> {
        if row.running {
            let frame = SPINNER[(self.started.elapsed().as_millis() / 100) as usize % SPINNER.len()];
            return Span::styled(format!(" {frame}RUNS "), Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD));
        }
        match row.status {
            Status::Passed => Span::styled(" PASS ", Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
            Status::Failed => Span::styled(" FAIL ", Style::default().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD)),
            Status::Skipped => Span::styled(" SKIP ", Style::default().fg(Color::Black).bg(DIM)),
            Status::Unknown => Span::styled("  ··  ", Style::default().fg(DIM)),
        }
    }

    fn folder_detail(&self, row: &Row) -> Text<'static> {
        let (passed, failed, skipped) = row.counts;
        let mut lines = vec![
            Line::styled(format!("{}/", row.full_name), Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
            Line::styled(
                format!("{} files · {passed} passed · {failed} failed · {skipped} skipped", row.files.len()),
                Style::default().fg(DIM),
            ),
            Line::styled("Press ⏎ to run every file in this folder", Style::default().fg(DIM)),
            Line::raw(""),
        ];
        let prefix = self.root.join(&row.full_name);
        let mut failures = Vec::new();
        for file in self.snapshot.files.iter().filter(|file| row.files.contains(&file.path)) {
            let relative = file.path.strip_prefix(&prefix).unwrap_or(&file.path).display().to_string();
            let file_row = Row { status: file.status, running: self.snapshot.running_files.contains(&file.path), ..row.clone() };
            let mut spans = vec![self.badge(&file_row), Span::raw(" "), Span::raw(relative), Span::raw("  ")];
            spans.extend(count_spans(counts(&file.nodes)));
            lines.push(Line::from(spans));
            for node in &file.nodes {
                collect_assertions(node, &mut failures);
            }
            failures.extend(file.unmatched.iter().cloned());
        }
        failures.retain(|assertion| assertion.status == Status::Failed);
        for assertion in &failures {
            lines.extend(assertion_lines(assertion, &self.root));
        }
        Text::from(lines)
    }

    fn detail_title(&self, row: Option<&Row>) -> String {
        let Some(row) = row else {
            return "Details".into();
        };
        if row.kind == RowKind::Folder {
            return format!("{}/", row.full_name);
        }
        let Some(file) = self.snapshot.files.iter().find(|file| file.path == row.file) else {
            return row.label.clone();
        };
        if row.kind == RowKind::File {
            return row.label.clone();
        }
        let mut names = Vec::new();
        let mut nodes = &file.nodes;
        for index in &row.node_path {
            let Some(node) = nodes.get(*index) else { break };
            names.push(node.name.clone());
            nodes = &node.children;
        }
        names.join(" › ")
    }

    fn detail_text(&self, row: Option<&Row>) -> Text<'static> {
        let Some(row) = row else {
            return Text::styled("Looking for tests…", Style::default().fg(DIM));
        };
        if row.kind == RowKind::Folder {
            return self.folder_detail(row);
        }
        let Some(file) = self.snapshot.files.iter().find(|file| file.path == row.file) else {
            return Text::default();
        };
        let mut lines = Vec::new();
        let mut assertions = Vec::new();
        match row.kind {
            RowKind::Folder => {}
            RowKind::File => {
                lines.push(Line::from(vec![self.badge(row), Span::raw(" "), Span::styled(row.label.clone(), Style::default().add_modifier(Modifier::BOLD))]));
                let (passed, failed, skipped) = row.counts;
                lines.push(Line::styled(format!("{passed} passed · {failed} failed · {skipped} skipped"), Style::default().fg(DIM)));
                if let Some(message) = &file.suite_message {
                    lines.push(Line::raw(""));
                    lines.push(Line::styled("● Test suite failed to run", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)));
                    lines.extend(message_lines(message, &self.root));
                }
                for node in &file.nodes {
                    collect_assertions(node, &mut assertions);
                }
                assertions.extend(file.unmatched.iter().cloned());
                assertions.retain(|assertion| assertion.status == Status::Failed);
            }
            RowKind::Describe | RowKind::Test => {
                if row.dynamic {
                    lines.push(Line::styled("each / dynamic name · Enter runs the whole file", Style::default().fg(DIM)));
                }
                if let Some(node) = node_at(&file.nodes, &row.node_path) {
                    collect_assertions(node, &mut assertions);
                }
                if assertions.is_empty() {
                    lines.push(Line::styled("Not run yet · press Enter to run", Style::default().fg(DIM)));
                }
            }
        }
        for assertion in &assertions {
            lines.extend(assertion_lines(assertion, &self.root));
        }
        if lines.first().is_some_and(|line| line.width() == 0) {
            lines.remove(0);
        }
        Text::from(lines)
    }

    fn selected_run(&self) -> Option<&RunRecord> {
        self.snapshot.history.get(self.history_list.selected().unwrap_or(0))
    }

    fn on_click(&mut self, column: u16, y: u16, rows: &[Row]) -> bool {
        let position = Position::new(column, y);
        let hit = self
            .targets
            .iter()
            .find(|target| target.y == y && (target.start..target.end).contains(&column))
            .map(|target| target.action);
        match hit {
            Some(ClickAction::Url(url)) => {
                let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
                let _ = Command::new(opener).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                return true;
            }
            Some(ClickAction::Key(code)) => return self.on_key(KeyEvent::new(code, KeyModifiers::NONE), rows),
            None => {}
        }

        if self.tree_area.contains(position) {
            self.set_focus(Focus::Tests);
            let Some(index) = list_index(self.tree_area, y, self.list.offset()).filter(|index| *index < rows.len()) else {
                return true;
            };
            let row = &rows[index];
            if self.is_double_click(Focus::Tests, index) {
                self.send(Request::Run { scope: scope_for(row) });
                return true;
            }
            if self.list.selected() != Some(index) {
                self.detail_scroll = 0;
            }
            self.list.select(Some(index));
            let fold_start = self.tree_area.x + 2 + fold_offset(row);
            if row.has_children && (fold_start..fold_start + 2).contains(&column) {
                let open = self.is_open(row);
                self.set_open(row, !open);
            }
        } else if self.history_area.contains(position) {
            self.set_focus(Focus::History);
            let len = self.snapshot.history.len();
            let Some(index) = list_index(self.history_area, y, self.history_list.offset()).filter(|index| *index < len) else {
                return true;
            };
            if self.is_double_click(Focus::History, index) {
                if let Some(scope) = self.snapshot.history[index].scope.clone() {
                    self.send(Request::Run { scope });
                }
                return true;
            }
            if self.history_list.selected() != Some(index) {
                self.detail_scroll = 0;
            }
            self.history_list.select(Some(index));
        } else if self.detail_area.contains(position) {
            self.set_focus(Focus::Details);
        }
        true
    }

    fn is_double_click(&mut self, focus: Focus, index: usize) -> bool {
        let now = Instant::now();
        let double = self.last_click.is_some_and(|(at, last_focus, last_index)| {
            last_focus == focus && last_index == index && now.duration_since(at) < Duration::from_millis(400)
        });
        self.last_click = if double { None } else { Some((now, focus, index)) };
        double
    }

    fn on_scroll(&mut self, column: u16, y: u16, delta: isize, rows: &[Row]) -> bool {
        let position = Position::new(column, y);
        if self.tree_area.contains(position) {
            let selected = self.list.selected().unwrap_or(0);
            self.move_selection(selected, delta, rows.len());
        } else if self.history_area.contains(position) {
            let len = self.snapshot.history.len();
            if len > 0 {
                let selected = self.history_list.selected().unwrap_or(0) as isize;
                self.history_list.select(Some((selected + delta).clamp(0, len as isize - 1) as usize));
                self.detail_scroll = 0;
            }
        } else if self.detail_area.contains(position) {
            self.detail_scroll = if delta > 0 {
                self.detail_scroll.saturating_add(delta as u16)
            } else {
                self.detail_scroll.saturating_sub(delta.unsigned_abs() as u16)
            };
        }
        true
    }

    fn footer_line(&self, width: u16) -> (Line<'static>, Vec<(u16, u16, ClickAction)>) {
        let mut left = Vec::new();
        let mut targets = Vec::new();
        let mut cursor: u16 = 0;
        let hints: &[(&str, &str)] = match self.focus {
            Focus::Tests => &[("1-3", "panel"), ("⏎", "run"), ("a", "all"), ("f", "failed"), ("v", "view"), ("/", "search"), ("F", "failed only"), ("q", "close")],
            Focus::History => &[("1-3", "panel"), ("j/k", "select"), ("⏎", "rerun"), ("a", "all"), ("f", "failed"), ("q", "close")],
            Focus::Details => &[("1-3", "panel"), ("j/k", "scroll"), ("^d/^u", "page"), ("a", "all"), ("f", "failed"), ("q", "close")],
        };
        for (key, label) in hints {
            let key_text = format!(" {key}");
            let label_text = format!(" {label} ");
            let span_width = (key_text.width() + label_text.width()) as u16;
            if let Some(code) = hint_key(key) {
                targets.push((cursor, cursor + span_width, ClickAction::Key(code)));
            }
            cursor += span_width;
            left.push(Span::styled(key_text, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)));
            left.push(Span::styled(label_text, Style::default().fg(DIM)));
        }
        let state = match (&self.snapshot.running, self.connected) {
            (_, false) => Span::styled("● disconnected ", Style::default().fg(Color::Red)),
            (Some(label), true) => {
                let frame = SPINNER[(self.started.elapsed().as_millis() / 100) as usize % SPINNER.len()];
                Span::styled(format!("{frame} running {label} "), Style::default().fg(Color::Yellow))
            }
            (None, true) => Span::styled("● idle ", Style::default().fg(Color::Green)),
        };
        let link_style = |color: Color| Style::default().fg(color).add_modifier(Modifier::UNDERLINED);
        let right = vec![
            Span::styled("Donate", link_style(Color::Magenta)),
            Span::raw("  "),
            Span::styled("Ask Question", link_style(ACCENT)),
            Span::raw("  "),
            Span::styled(env!("CARGO_PKG_VERSION"), Style::default().fg(DIM)),
            Span::raw("  "),
            state,
        ];
        let right_width: usize = right.iter().map(|span| span.content.width()).sum();
        let start = (width as usize).saturating_sub(right_width) as u16;
        targets.push((start, start + 6, ClickAction::Url(DONATE_URL)));
        targets.push((start + 8, start + 20, ClickAction::Url(QUESTION_URL)));
        (justify(left, right, width as usize), targets)
    }
}

fn selected_line(line: Line<'static>) -> Line<'static> {
    let spans = line
        .spans
        .into_iter()
        .map(|span| {
            let style = if span.style.bg.is_none() { span.style.bg(SELECTED_BG) } else { span.style };
            Span::styled(span.content, style)
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn hint_key(key: &str) -> Option<KeyCode> {
    match key {
        "⏎" => Some(KeyCode::Enter),
        "a" | "f" | "v" | "/" | "F" | "q" => key.chars().next().map(KeyCode::Char),
        _ => None,
    }
}

fn list_index(area: Rect, y: u16, offset: usize) -> Option<usize> {
    let first = area.y + 1;
    let last = area.y + area.height.saturating_sub(1);
    (first..last).contains(&y).then(|| offset + usize::from(y - first))
}

fn fold_offset(row: &Row) -> u16 {
    let indent = row.indent as u16 * 2;
    match row.kind {
        RowKind::Folder | RowKind::File => indent,
        RowKind::Describe | RowKind::Test => indent + 2 + row.node_path.len().saturating_sub(1) as u16 * 2,
    }
}

const HISTORY_ROWS: usize = 6;
const DONATE_URL: &str = "https://github.com/sponsors/doominkim";
const QUESTION_URL: &str = "https://github.com/doominkim/herdr-jest/issues";
const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const GUIDE: Color = Color::Indexed(238);
const SELECTED_BG: Color = Color::Indexed(24);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn panel(title: Line<'static>, focused: bool) -> Block<'static> {
    let border = if focused { ACCENT } else { GUIDE };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(title.style(Style::default().add_modifier(Modifier::BOLD)))
}

fn justify(mut left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let used: usize = left.iter().chain(&right).map(|span| span.content.width()).sum();
    left.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    left.extend(right);
    Line::from(left)
}

fn count_spans((passed, failed, skipped): (usize, usize, usize)) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if failed > 0 {
        spans.push(Span::styled(format!("{failed}✗ "), Style::default().fg(Color::Red)));
    }
    if passed > 0 {
        spans.push(Span::styled(format!("{passed}✓ "), Style::default().fg(Color::Green)));
    }
    if skipped > 0 {
        spans.push(Span::styled(format!("{skipped}○ "), Style::default().fg(DIM)));
    }
    spans
}

fn run_detail(run: &RunRecord) -> Text<'static> {
    let field = |name: &str, value: String, style: Style| {
        Line::from(vec![Span::styled(format!("{name:<9}"), Style::default().fg(DIM)), Span::styled(value, style)])
    };
    let mut lines = vec![
        field("Scope", run.label.clone(), Style::default().add_modifier(Modifier::BOLD)),
        field("Branch", run.branch.clone().unwrap_or_else(|| "-".into()), Style::default().fg(ACCENT)),
        field("Finished", local_time(run.finished_at), Style::default()),
        field("Duration", run.duration_ms.map(format_duration).unwrap_or_else(|| "-".into()), Style::default()),
        Line::raw(""),
    ];
    if run.error {
        lines.push(Line::styled("⚠ Jest exited without results. See the error bar.", Style::default().fg(Color::Red)));
    } else {
        lines.push(field("Passed", run.passed.to_string(), Style::default().fg(Color::Green)));
        lines.push(field(
            "Failed",
            run.failed.to_string(),
            if run.failed > 0 { Style::default().fg(Color::Red).add_modifier(Modifier::BOLD) } else { Style::default() },
        ));
    }
    if run.scope.is_some() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("Press ⏎ to run this scope again", Style::default().fg(DIM)));
    }
    Text::from(lines)
}

fn history_line(run: &RunRecord, width: usize) -> Line<'static> {
    let mut right = Vec::new();
    if run.error {
        right.push(Span::styled("⚠ error ", Style::default().fg(Color::Red)));
    } else {
        right.extend(count_spans((run.passed as usize, run.failed as usize, 0)));
    }
    if let Some(ms) = run.duration_ms {
        right.push(Span::styled(format!("{:>7} ", format_duration(ms)), Style::default().fg(DIM)));
    }
    let right_width: usize = right.iter().map(|span| span.content.width()).sum();

    let time = format!(" {}  ", local_time(run.finished_at));
    let branch = run.branch.clone().unwrap_or_else(|| "-".into());
    let branch = format!("{}  ", truncate(&branch, 18));
    let label_width = width.saturating_sub(time.width() + branch.width() + right_width + 1);
    let label_color = if run.error || run.failed > 0 { Color::Red } else { Color::Reset };
    let left = vec![
        Span::styled(time, Style::default().fg(DIM)),
        Span::styled(branch, Style::default().fg(ACCENT)),
        Span::styled(ellipsize_middle(&run.label, label_width), Style::default().fg(label_color)),
    ];
    justify(left, right, width)
}

fn truncate(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

fn ellipsize_middle(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    if max <= 1 {
        return "…".repeat(max);
    }
    let keep = max - 1;
    let head_budget = keep / 2;
    let tail_budget = keep - head_budget;
    let mut head = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > head_budget {
            break;
        }
        head.push(c);
        used += w;
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0;
    for c in text.chars().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > tail_budget {
            break;
        }
        tail.push(c);
        used += w;
    }
    tail.reverse();
    format!("{head}…{}", tail.into_iter().collect::<String>())
}

fn local_time(unix_secs: u64) -> String {
    let time = unix_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&time, &mut tm) };
    format!("{:02}/{:02} {:02}:{:02}", tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
}

fn format_duration(ms: u64) -> String {
    if ms < 1000 { format!("{ms} ms") } else { format!("{:.1} s", ms as f64 / 1000.0) }
}

fn scope_for(row: &Row) -> Scope {
    match row.kind {
        RowKind::Folder => Scope::Files(row.files.clone()),
        RowKind::File => Scope::File(row.file.clone()),
        _ if row.dynamic => Scope::File(row.file.clone()),
        RowKind::Describe => Scope::Describe { file: row.file.clone(), full_name: row.full_name.clone() },
        RowKind::Test => Scope::Test { file: row.file.clone(), full_name: row.full_name.clone() },
    }
}

fn icon(status: Status) -> (&'static str, Color) {
    match status {
        Status::Passed => ("✓", Color::Green),
        Status::Failed => ("✗", Color::Red),
        Status::Skipped => ("○", DIM),
        Status::Unknown => ("·", DIM),
    }
}

fn folder_status(statuses: impl Iterator<Item = Status>) -> Status {
    let mut seen = Status::Unknown;
    for status in statuses {
        match (seen, status) {
            (_, Status::Failed) => return Status::Failed,
            (_, Status::Passed) => seen = Status::Passed,
            (Status::Unknown, Status::Skipped) => seen = Status::Skipped,
            _ => {}
        }
    }
    seen
}

fn counts(nodes: &[NodeView]) -> (usize, usize, usize) {
    let mut totals = (0, 0, 0);
    for node in nodes {
        match node.kind {
            NodeKind::Test if !node.assertions.is_empty() => {
                for assertion in &node.assertions {
                    match assertion.status {
                        Status::Passed => totals.0 += 1,
                        Status::Failed => totals.1 += 1,
                        Status::Skipped => totals.2 += 1,
                        Status::Unknown => {}
                    }
                }
            }
            NodeKind::Test => {
                if node.status == Status::Skipped {
                    totals.2 += 1;
                }
            }
            NodeKind::Describe => {
                let (p, f, s) = counts(&node.children);
                totals = (totals.0 + p, totals.1 + f, totals.2 + s);
            }
        }
    }
    totals
}

fn expand_failed_nodes(expanded: &mut BTreeSet<String>, parent_key: &str, nodes: &[NodeView]) {
    for (index, node) in nodes.iter().enumerate() {
        if node.status == Status::Failed && !node.children.is_empty() {
            let key = format!("{parent_key}/{index}");
            expanded.insert(key.clone());
            expand_failed_nodes(expanded, &key, &node.children);
        }
    }
}

fn node_at<'a>(nodes: &'a [NodeView], node_path: &[usize]) -> Option<&'a NodeView> {
    let (first, rest) = node_path.split_first()?;
    let node = nodes.get(*first)?;
    if rest.is_empty() {
        return Some(node);
    }
    node_at(&node.children, rest)
}

fn collect_assertions(node: &NodeView, out: &mut Vec<Assertion>) {
    out.extend(node.assertions.iter().cloned());
    for child in &node.children {
        collect_assertions(child, out);
    }
}

fn assertion_lines(assertion: &Assertion, root: &Path) -> Vec<Line<'static>> {
    let duration = assertion
        .duration_ms
        .map(|ms| Span::styled(format!("  {}", format_duration(ms)), Style::default().fg(DIM)))
        .unwrap_or_else(|| Span::raw(""));
    let header = match assertion.status {
        Status::Failed => Span::styled(format!("● {}", assertion.full_name), Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Status::Passed => Span::styled(format!("✓ {}", assertion.full_name), Style::default().fg(Color::Green)),
        Status::Skipped => Span::styled(format!("○ {}", assertion.full_name), Style::default().fg(DIM)),
        Status::Unknown => Span::styled(format!("· {}", assertion.full_name), Style::default().fg(DIM)),
    };
    let mut lines = vec![Line::raw(""), Line::from(vec![header, duration])];
    for message in &assertion.failure_messages {
        lines.push(Line::raw(""));
        lines.extend(message_lines(message, root));
    }
    lines
}

fn message_lines(message: &str, root: &Path) -> Vec<Line<'static>> {
    let root_prefix = format!("{}/", root.display());
    let mut lines = Vec::new();
    let mut shown_frame = false;
    for line in message.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("at ") {
            if shown_frame || line.contains("/node_modules/") || line.contains("(<anonymous>)") {
                continue;
            }
            shown_frame = true;
            let frame = trimmed.strip_prefix("at ").unwrap_or(trimmed);
            let location = frame.rsplit_once('(').map(|(_, rest)| rest.trim_end_matches(')')).unwrap_or(frame);
            let location = location.strip_prefix(&root_prefix).unwrap_or(location).to_string();
            lines.push(Line::raw(""));
            lines.push(Line::styled(format!("  at {location}"), Style::default().fg(DIM)));
            continue;
        }
        let text = trimmed.strip_prefix("Error: ").unwrap_or(trimmed);
        let indent = &line[..line.len() - trimmed.len()];
        let style = if text.starts_with("expect(") {
            Style::default().add_modifier(Modifier::BOLD)
        } else if text.starts_with("Expected") || text.starts_with("- ") {
            Style::default().fg(Color::Green)
        } else if text.starts_with("Received") || text.starts_with("+ ") {
            Style::default().fg(Color::Red)
        } else {
            Style::default()
        };
        lines.push(Line::styled(format!("  {indent}{text}"), style));
    }
    while lines.last().is_some_and(|line| line.width() == 0) {
        lines.pop();
    }
    lines
}

fn connect_or_spawn(root: &Path) -> Option<UnixStream> {
    let socket = paths::socket_path(root);
    if let Ok(stream) = UnixStream::connect(&socket) {
        return Some(stream);
    }
    spawn_daemon(root).ok()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(stream) = UnixStream::connect(&socket) {
            return Some(stream);
        }
        thread::sleep(Duration::from_millis(100));
    }
    None
}

fn spawn_daemon(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(paths::state_dir())?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(paths::log_file(root))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["daemon", "--root"])
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn()?;
    Ok(())
}

fn wait_for_key() {
    let _ = crossterm::terminal::enable_raw_mode();
    let _ = event::read();
    let _ = crossterm::terminal::disable_raw_mode();
}
