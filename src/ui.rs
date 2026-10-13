use crate::args::Scope;
use crate::discover::NodeKind;
use crate::paths;
use crate::protocol::{FileView, NodeView, Request, Snapshot};
use crate::results::{Assertion, Status};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    File,
    Describe,
    Test,
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
}

pub fn run(cwd: &Path) -> std::io::Result<()> {
    let Some(root) = paths::find_root(cwd) else {
        eprintln!("herdr-jest: {} 위로 jest 설정을 찾지 못했어", cwd.display());
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

    let mut app = App {
        root,
        connected: stream.is_some(),
        snapshot,
        expanded: BTreeSet::new(),
        list: ListState::default().with_selected(Some(0)),
        failed_only: false,
        query: String::new(),
        searching: false,
        detail_scroll: 0,
        stream,
    };
    app.expand_failed();

    let mut terminal = ratatui::init();
    let result = app.event_loop(&mut terminal, rx);
    ratatui::restore();
    result
}

impl App {
    fn event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal, rx: Receiver<Snapshot>) -> std::io::Result<()> {
        loop {
            while let Ok(snapshot) = rx.try_recv() {
                let first_results = self.snapshot.runs_completed == 0 && snapshot.runs_completed > 0;
                self.snapshot = snapshot;
                if first_results {
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
            let Event::Key(key) = event::read()? else {
                continue;
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
        let selected = self.list.selected().unwrap_or(0);
        let row = rows.get(selected);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::F(2) => return false,
            KeyCode::Char('d') if ctrl => self.detail_scroll = self.detail_scroll.saturating_add(10),
            KeyCode::Char('u') if ctrl => self.detail_scroll = self.detail_scroll.saturating_sub(10),
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(selected, 1, rows.len()),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(selected, -1, rows.len()),
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(row) = row.filter(|row| row.has_children) {
                    self.expanded.insert(row.key.clone());
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.collapse_or_parent(selected, rows),
            KeyCode::Enter => {
                if let Some(row) = row {
                    self.send(Request::Run { scope: scope_for(row) });
                }
            }
            KeyCode::Char('a') => self.send(Request::Run { scope: Scope::All }),
            KeyCode::Char('f') => self.send(Request::RunFailed),
            KeyCode::Char('F') => {
                self.failed_only = !self.failed_only;
                self.list.select(Some(0));
            }
            KeyCode::Char('/') => {
                self.searching = true;
                self.query.clear();
            }
            KeyCode::Char('X') => {
                self.send(Request::Stop);
                return false;
            }
            _ => {}
        }
        true
    }

    fn move_selection(&mut self, selected: usize, delta: isize, len: usize) {
        if len == 0 {
            return;
        }
        let next = (selected as isize + delta).clamp(0, len as isize - 1) as usize;
        self.list.select(Some(next));
        self.detail_scroll = 0;
    }

    fn collapse_or_parent(&mut self, selected: usize, rows: &[Row]) {
        let Some(row) = rows.get(selected) else {
            return;
        };
        if self.expanded.remove(&row.key) {
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
        let query = self.query.to_lowercase();
        let running: BTreeSet<&PathBuf> = self.snapshot.running_files.iter().collect();
        let mut rows = Vec::new();
        for file in &self.snapshot.files {
            if self.failed_only && file.status != Status::Failed {
                continue;
            }
            let relative = file.path.strip_prefix(&self.root).unwrap_or(&file.path).display().to_string();
            let file_matches = query.is_empty() || relative.to_lowercase().contains(&query);
            let mut children = Vec::new();
            let key = file.path.display().to_string();
            let forced_open = !query.is_empty() && !file_matches;
            let open = self.expanded.contains(&key) || forced_open;
            let any_child = self.node_rows(file, &file.nodes, &key, &[], open, file_matches, &mut children);
            if !file_matches && !any_child {
                continue;
            }
            let (passed, failed) = counts(&file.nodes);
            rows.push(Row {
                depth: 0,
                key,
                label: format!("{relative}  {passed}✓ {failed}✗"),
                status: file.status,
                running: running.contains(&file.path),
                kind: RowKind::File,
                file: file.path.clone(),
                full_name: String::new(),
                node_path: Vec::new(),
                dynamic: false,
                has_children: !file.nodes.is_empty(),
            });
            rows.extend(children);
        }
        rows
    }

    #[allow(clippy::too_many_arguments)]
    fn node_rows(
        &self,
        file: &FileView,
        nodes: &[NodeView],
        parent_key: &str,
        parent_path: &[usize],
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
            let child_matches = self.node_rows(file, &node.children, &key, &node_path, visible && open, self_matches, &mut children);
            if !self_matches && !child_matches {
                continue;
            }
            any = true;
            if visible {
                out.push(Row {
                    depth: node_path.len(),
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
                });
                out.extend(children);
            }
        }
        any
    }

    fn draw(&mut self, frame: &mut Frame, rows: &[Row]) {
        let [body, status_bar] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
        let [tree_area, detail_area] =
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(body);

        let items: Vec<ListItem> = rows.iter().map(|row| ListItem::new(self.row_line(row))).collect();
        let title = if self.searching || !self.query.is_empty() {
            format!(" 테스트  /{} ", self.query)
        } else if self.failed_only {
            " 테스트 (실패만) ".to_string()
        } else {
            " 테스트 ".to_string()
        };
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
        frame.render_stateful_widget(list, tree_area, &mut self.list);

        let selected = self.list.selected().and_then(|index| rows.get(index));
        let detail = Paragraph::new(self.detail_text(selected))
            .block(Block::default().borders(Borders::ALL).title(" 상세 "))
            .wrap(Wrap { trim: false })
            .scroll((self.detail_scroll, 0));
        frame.render_widget(detail, detail_area);

        frame.render_widget(Paragraph::new(self.status_line()), status_bar);
    }

    fn row_line(&self, row: &Row) -> Line<'static> {
        let (icon, color) = if row.running {
            ("◐", Color::Yellow)
        } else {
            icon(row.status)
        };
        let indent = "  ".repeat(row.depth);
        let fold = match (row.has_children, self.expanded.contains(&row.key)) {
            (false, _) => "  ",
            (true, true) => "▾ ",
            (true, false) => "▸ ",
        };
        let label_style = match row.kind {
            RowKind::File => Style::default().add_modifier(Modifier::BOLD),
            RowKind::Describe => Style::default().fg(Color::Cyan),
            RowKind::Test => Style::default(),
        };
        Line::from(vec![
            Span::raw(indent),
            Span::raw(fold),
            Span::styled(format!("{icon} "), Style::default().fg(color)),
            Span::styled(row.label.clone(), label_style),
        ])
    }

    fn detail_text(&self, row: Option<&Row>) -> Text<'static> {
        let Some(row) = row else {
            return Text::from("테스트를 찾는 중…");
        };
        let file = self.snapshot.files.iter().find(|file| file.path == row.file);
        let mut lines = Vec::new();
        match row.kind {
            RowKind::File => {
                lines.push(Line::styled(row.label.clone(), Style::default().add_modifier(Modifier::BOLD)));
                if let Some(message) = file.and_then(|file| file.suite_message.clone()) {
                    lines.push(Line::raw(""));
                    lines.extend(message_lines(&message));
                }
                for assertion in file.map(|file| file.unmatched.clone()).unwrap_or_default() {
                    lines.extend(assertion_lines(&assertion));
                }
            }
            RowKind::Describe | RowKind::Test => {
                let node = file.and_then(|file| node_at(&file.nodes, &row.node_path));
                lines.push(Line::styled(row.full_name.clone(), Style::default().add_modifier(Modifier::BOLD)));
                if row.dynamic {
                    lines.push(Line::styled("(each/동적 이름: Enter는 파일 단위로 실행)", Style::default().fg(Color::DarkGray)));
                }
                if let Some(node) = node {
                    let mut assertions = Vec::new();
                    collect_assertions(node, &mut assertions);
                    if assertions.is_empty() {
                        lines.push(Line::styled("아직 실행 결과 없음", Style::default().fg(Color::DarkGray)));
                    }
                    for assertion in assertions {
                        lines.extend(assertion_lines(&assertion));
                    }
                }
            }
        }
        Text::from(lines)
    }

    fn status_line(&self) -> Line<'static> {
        let (passed, failed) = self
            .snapshot
            .files
            .iter()
            .map(|file| counts(&file.nodes))
            .fold((0, 0), |(p, f), (fp, ff)| (p + fp, f + ff));
        let state = match (&self.snapshot.running, self.connected) {
            (_, false) => Span::styled(" 데몬 연결 안 됨 ", Style::default().fg(Color::Black).bg(Color::Red)),
            (Some(label), true) => Span::styled(format!(" 실행 중: {label} "), Style::default().fg(Color::Black).bg(Color::Yellow)),
            (None, true) => Span::styled(" 대기 ", Style::default().fg(Color::Black).bg(Color::Green)),
        };
        let mut spans = vec![
            state,
            Span::raw(format!(" {passed}✓ {failed}✗ · ")),
            Span::styled(
                "Enter 실행 a 전체 f 실패만 / 검색 F 실패 보기 X 정지 q 닫기",
                Style::default().fg(Color::DarkGray),
            ),
        ];
        if let Some(error) = &self.snapshot.last_error {
            let first = error.lines().find(|line| !line.trim().is_empty()).unwrap_or_default();
            spans.push(Span::styled(format!("  ⚠ {first}"), Style::default().fg(Color::Red)));
        }
        Line::from(spans)
    }
}

fn scope_for(row: &Row) -> Scope {
    match row.kind {
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
        Status::Skipped => ("○", Color::DarkGray),
        Status::Unknown => ("·", Color::DarkGray),
    }
}

fn counts(nodes: &[NodeView]) -> (usize, usize) {
    let mut passed = 0;
    let mut failed = 0;
    for node in nodes {
        match node.kind {
            NodeKind::Test => match node.status {
                Status::Passed => passed += 1,
                Status::Failed => failed += 1,
                _ => {}
            },
            NodeKind::Describe => {
                let (p, f) = counts(&node.children);
                passed += p;
                failed += f;
            }
        }
    }
    (passed, failed)
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

fn assertion_lines(assertion: &Assertion) -> Vec<Line<'static>> {
    let (icon, color) = icon(assertion.status);
    let duration = assertion.duration_ms.map(|ms| format!(" ({ms}ms)")).unwrap_or_default();
    let mut lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled(format!("{icon} "), Style::default().fg(color)),
            Span::raw(assertion.full_name.clone()),
            Span::styled(duration, Style::default().fg(Color::DarkGray)),
        ]),
    ];
    for message in &assertion.failure_messages {
        lines.extend(message_lines(message));
    }
    lines
}

fn message_lines(message: &str) -> Vec<Line<'static>> {
    message
        .lines()
        .filter(|line| !(line.trim_start().starts_with("at ") && line.contains("/node_modules/")))
        .map(|line| {
            let trimmed = line.trim_start();
            let style = if trimmed.starts_with("at ") {
                Style::default().fg(Color::DarkGray)
            } else if trimmed.starts_with("Expected") || trimmed.starts_with("- Expected") || trimmed.starts_with('-') {
                Style::default().fg(Color::Green)
            } else if trimmed.starts_with("Received") || trimmed.starts_with("+ Received") || trimmed.starts_with('+') {
                Style::default().fg(Color::Red)
            } else {
                Style::default()
            };
            Line::styled(format!("  {line}"), style)
        })
        .collect()
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
