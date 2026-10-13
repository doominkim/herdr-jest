use crate::args::Scope;
use crate::protocol::{Request, Snapshot, file_views};
use crate::results::Model;
use crate::runner::{self, RunOutput};
use crate::{alert, config, discover, paths};
use std::collections::{BTreeSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

enum Event {
    Connected(u64, UnixStream),
    Disconnected(u64),
    Request(Request),
    Discovered(Result<Vec<PathBuf>, String>),
    FileDone(PathBuf),
    RunFinished(RunOutput),
    Tick,
}

struct Daemon {
    root: PathBuf,
    command: Vec<String>,
    model: Model,
    test_files: BTreeSet<PathBuf>,
    queue: VecDeque<Scope>,
    running: Option<String>,
    running_files: BTreeSet<PathBuf>,
    runs_completed: u64,
    last_error: Option<String>,
    clients: Vec<(u64, UnixStream)>,
    tx: Sender<Event>,
}

pub fn serve(root: &Path) -> std::io::Result<()> {
    let root = root.canonicalize()?;
    std::fs::create_dir_all(paths::state_dir())?;
    let socket = paths::socket_path(&root);
    if UnixStream::connect(&socket).is_ok() {
        return Ok(());
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;

    let (tx, rx) = mpsc::channel();
    spawn_acceptor(listener, tx.clone());
    spawn_ticker(tx.clone());

    let saved = std::fs::read(paths::state_file(&root))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Snapshot>(&bytes).ok())
        .unwrap_or_default();
    let mut model = Model::default();
    model.restore(&saved.files);

    let mut daemon = Daemon {
        command: config::command_for(&root),
        root,
        model,
        test_files: BTreeSet::new(),
        queue: VecDeque::new(),
        running: None,
        running_files: BTreeSet::new(),
        runs_completed: saved.runs_completed,
        last_error: None,
        clients: Vec::new(),
        tx,
    };
    daemon.discover();

    for event in rx {
        let keep_going = daemon.handle(event);
        if !keep_going {
            break;
        }
    }
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

impl Daemon {
    fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::Connected(id, mut stream) => {
                if send(&mut stream, &self.snapshot()) {
                    self.clients.push((id, stream));
                }
                self.discover();
            }
            Event::Disconnected(id) => self.clients.retain(|(client, _)| *client != id),
            Event::Request(Request::Run { scope }) => self.enqueue(scope),
            Event::Request(Request::RunFailed) => {
                let failed = self.model.failed();
                if !failed.is_empty() {
                    self.enqueue(Scope::Failed(failed));
                }
            }
            Event::Request(Request::Stop) => return false,
            Event::Discovered(Ok(files)) => {
                self.test_files = files.into_iter().collect();
                let known: Vec<PathBuf> = self.model.files().map(Path::to_path_buf).collect();
                for file in known.iter().filter(|file| !self.test_files.contains(*file)) {
                    self.model.remove_file(file);
                }
                for file in self.test_files.clone() {
                    self.reparse(&file);
                }
                self.broadcast();
            }
            Event::Discovered(Err(error)) => {
                self.last_error = Some(error);
                self.broadcast();
            }
            Event::FileDone(file) => {
                self.running_files.remove(&file);
                self.broadcast();
            }
            Event::RunFinished(output) => self.on_run_finished(output),
            Event::Tick => {
                if !self.root.exists() {
                    return false;
                }
            }
        }
        true
    }

    fn discover(&self) {
        let tx = self.tx.clone();
        let root = self.root.clone();
        let command = self.command.clone();
        thread::spawn(move || {
            let _ = tx.send(Event::Discovered(runner::list_tests(&root, &command)));
        });
    }

    fn reparse(&mut self, file: &Path) {
        match std::fs::read_to_string(file) {
            Ok(source) => self.model.set_tree(file.to_path_buf(), discover::parse(file, &source)),
            Err(_) => self.model.remove_file(file),
        }
    }

    fn enqueue(&mut self, scope: Scope) {
        if let Some(file) = scope_file(&scope) {
            self.reparse(&file);
        }
        if !self.queue.contains(&scope) {
            self.queue.push_back(scope);
        }
        self.start_next();
    }

    fn start_next(&mut self) {
        if self.running.is_some() {
            return;
        }
        let Some(scope) = self.queue.pop_front() else {
            return;
        };
        self.running = Some(label(&scope, &self.root));
        self.running_files = match &scope {
            Scope::All => self.test_files.clone(),
            Scope::File(file) | Scope::SavedTestFile(file) => [file.clone()].into(),
            Scope::Describe { file, .. } | Scope::Test { file, .. } => [file.clone()].into(),
            Scope::Failed(failures) => failures.iter().map(|(file, _)| file.clone()).collect(),
            Scope::SavedSource(_) => BTreeSet::new(),
        };
        self.broadcast();

        let tx = self.tx.clone();
        let root = self.root.clone();
        let command = self.command.clone();
        let output_file = paths::run_output(&root);
        thread::spawn(move || {
            let progress = tx.clone();
            let output = runner::run(&root, &command, &scope, &output_file, |file| {
                let _ = progress.send(Event::FileDone(file));
            });
            let _ = tx.send(Event::RunFinished(output));
        });
    }

    fn on_run_finished(&mut self, output: RunOutput) {
        self.last_error = output.error;
        let mut newly_failed = Vec::new();
        if let Some(report) = output.report {
            match self.model.apply(&report) {
                Ok(failed) => newly_failed = failed,
                Err(error) => self.last_error = Some(format!("jest report: {error}")),
            }
        }
        self.running = None;
        self.running_files.clear();
        self.runs_completed += 1;
        self.persist();
        self.broadcast();
        if self.clients.is_empty() {
            alert::newly_failed(&newly_failed);
        }
        self.start_next();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            root: self.root.clone(),
            running: self.running.clone(),
            running_files: self.running_files.iter().cloned().collect(),
            runs_completed: self.runs_completed,
            last_error: self.last_error.clone(),
            files: file_views(&self.model),
        }
    }

    fn broadcast(&mut self) {
        let snapshot = self.snapshot();
        self.clients.retain_mut(|(_, client)| send(client, &snapshot));
    }

    fn persist(&self) {
        let target = paths::state_file(&self.root);
        let temporary = target.with_extension("json.tmp");
        let Ok(json) = serde_json::to_vec(&self.snapshot()) else {
            return;
        };
        if std::fs::write(&temporary, json).is_ok() {
            let _ = std::fs::rename(&temporary, &target);
        }
    }
}

fn send(stream: &mut UnixStream, snapshot: &Snapshot) -> bool {
    let Ok(mut line) = serde_json::to_vec(snapshot) else {
        return false;
    };
    line.push(b'\n');
    stream.write_all(&line).is_ok()
}

fn spawn_acceptor(listener: UnixListener, tx: Sender<Event>) {
    thread::spawn(move || {
        for (id, stream) in (0u64..).zip(listener.incoming().map_while(Result::ok)) {
            let Ok(reader) = stream.try_clone() else {
                continue;
            };
            if tx.send(Event::Connected(id, stream)).is_err() {
                return;
            }
            let requests = tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(reader).lines().map_while(Result::ok) {
                    if let Ok(request) = serde_json::from_str::<Request>(&line) {
                        let _ = requests.send(Event::Request(request));
                    }
                }
                let _ = requests.send(Event::Disconnected(id));
            });
        }
    });
}

fn spawn_ticker(tx: Sender<Event>) {
    thread::spawn(move || {
        while tx.send(Event::Tick).is_ok() {
            thread::sleep(Duration::from_secs(2));
        }
    });
}

fn scope_file(scope: &Scope) -> Option<PathBuf> {
    match scope {
        Scope::File(file) | Scope::Describe { file, .. } | Scope::Test { file, .. } => Some(file.clone()),
        _ => None,
    }
}

fn label(scope: &Scope, root: &Path) -> String {
    let short = |file: &Path| file.strip_prefix(root).unwrap_or(file).display().to_string();
    match scope {
        Scope::All => "전체".into(),
        Scope::File(file) => short(file),
        Scope::Describe { full_name, .. } | Scope::Test { full_name, .. } => full_name.clone(),
        Scope::Failed(failures) => format!("실패 {}개", failures.len()),
        Scope::SavedTestFile(file) => format!("저장: {}", short(file)),
        Scope::SavedSource(file) => format!("관련: {}", short(file)),
    }
}
