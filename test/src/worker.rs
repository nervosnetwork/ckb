use crate::logger::{SpecLogGuard, read_runner_log_path};
use crate::utils::{SpecRunDirGuard, TempPathBuf, nodes_panicked};
use crate::{Node, Spec, extract_panic_message, format_duration};
use ckb_channel::{Receiver, Sender, unbounded};
use ckb_logger::{debug, error, info};
use ckb_util::Mutex;
use std::any::Any;
use std::collections::BTreeSet;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, BufRead, Write as IoWrite};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// A single scheduled execution of a [`Spec`].
pub struct ScheduledSpec {
    pub spec: Box<dyn Spec>,
    pub base_name: String,
    pub display_name: String,
    pub iteration: usize,
    pub total_iterations: usize,
}

impl ScheduledSpec {
    pub fn new(spec: Box<dyn Spec>, iteration: usize, total_iterations: usize) -> Self {
        let base_name = spec.name().to_string();
        let display_name = if total_iterations > 1 {
            format!("{base_name} [{iteration}/{total_iterations}]")
        } else {
            base_name.clone()
        };
        Self {
            spec,
            base_name,
            display_name,
            iteration,
            total_iterations,
        }
    }

    pub fn dir_label(&self) -> String {
        if self.total_iterations > 1 {
            format!("{}-r{}", self.base_name, self.iteration)
        } else {
            self.base_name.clone()
        }
    }
}

/// Shared runtime configuration for workers.
#[derive(Clone)]
pub struct WorkerConfig {
    pub seed: u64,
    pub pause_on_fail: bool,
    pub rerun_cmd_prefix: String,
}

/// Commands
#[derive(PartialEq, Eq)]
pub enum Command {
    Shutdown,
    StartSequencial,
}

/// Notify from worker
pub enum Notify {
    Start {
        spec_name: String,
        base_name: String,
        spec_dir: PathBuf,
        started_at: Instant,
    },
    Done {
        spec_name: String,
        base_name: String,
        duration: Duration,
        finished_at: Instant,
        spec_dir: TempPathBuf,
        node_paths: Vec<TempPathBuf>,
    },
    Error {
        spec_error: Box<dyn Any + Send>,
        spec_name: String,
        base_name: String,
        duration: Duration,
        finished_at: Instant,
        spec_dir: Option<TempPathBuf>,
        report_path: Option<PathBuf>,
        runner_log_path: Option<PathBuf>,
        node_log_paths: Vec<PathBuf>,
        node_paths: Vec<TempPathBuf>,
    },
    Panick {
        spec_name: String,
        base_name: String,
        duration: Duration,
        finished_at: Instant,
        spec_dir: TempPathBuf,
        report_path: Option<PathBuf>,
        runner_log_path: Option<PathBuf>,
        node_log_paths: Vec<PathBuf>,
        node_paths: Vec<TempPathBuf>,
    },
    Stop,
}

struct WorkerStopGuard {
    outbox: Sender<Notify>,
}

impl Drop for WorkerStopGuard {
    fn drop(&mut self) {
        let _ = self.outbox.send(Notify::Stop);
    }
}

/// Worker
pub struct Worker {
    id: usize,
    tasks: Arc<Mutex<Vec<ScheduledSpec>>>,
    inbox: Receiver<Command>,
    outbox: Sender<Notify>,
    config: WorkerConfig,

    sequencial_tasks: Arc<Mutex<Vec<ScheduledSpec>>>,
    sequencial_worker: bool,
}

impl Clone for Worker {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            tasks: Arc::clone(&self.tasks),
            inbox: self.inbox.clone(),
            outbox: self.outbox.clone(),
            config: self.config.clone(),
            sequencial_tasks: Arc::clone(&self.sequencial_tasks),
            sequencial_worker: self.sequencial_worker,
        }
    }
}

pub(crate) const SEQUENCIAL_TASKS: &[&str] = &["RandomlyKill", "SyncChurn"];

fn pause_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

impl Worker {
    pub fn new(
        id: usize,
        tasks: Arc<Mutex<Vec<ScheduledSpec>>>,
        sequencial_tasks: Arc<Mutex<Vec<ScheduledSpec>>>,
        inbox: Receiver<Command>,
        outbox: Sender<Notify>,
        config: WorkerConfig,
    ) -> Self {
        Worker {
            id,
            tasks,
            inbox,
            outbox,
            config,
            sequencial_tasks,
            sequencial_worker: false,
        }
    }

    /// start handle tasks
    pub fn start(self) -> JoinHandle<()> {
        let name = format!("worker-{}", self.id);
        thread::Builder::new()
            .name(name)
            .spawn(move || {
                // Always emit exactly one `Notify::Stop` when this worker exits (including on panic)
                // so the coordinator can never hang waiting for a dead worker.
                let _stop_guard = WorkerStopGuard {
                    outbox: self.outbox.clone(),
                };
                let mut start_sequencial_task = false;

                loop {
                    loop {
                        match self.inbox.try_recv() {
                            Ok(Command::StartSequencial) => {
                                start_sequencial_task = true;
                            }
                            Ok(Command::Shutdown) => {
                                return;
                            }
                            Err(err) if err.is_empty() => break,
                            Err(err) => std::panic::panic_any(err),
                        }
                    }

                    // pick a spec to run
                    let task = self.tasks.lock().pop();
                    match task {
                        Some(task) => {
                            self.run_spec(&task, 0, Instant::now());
                        }
                        None => {
                            if self.sequencial_worker {
                                if !start_sequencial_task && self.sequencial_tasks.lock().is_empty()
                                {
                                    return;
                                }
                                while !start_sequencial_task {
                                    match self.inbox.recv() {
                                        Ok(Command::StartSequencial) => {
                                            start_sequencial_task = true;
                                        }
                                        Ok(Command::Shutdown) => {
                                            return;
                                        }
                                        Err(err) => {
                                            std::panic::panic_any(err);
                                        }
                                    }
                                }
                                match self.sequencial_tasks.lock().pop() {
                                    Some(task) => {
                                        self.run_spec(&task, 0, Instant::now());
                                    }
                                    None => {
                                        debug!("sequential worker has no task to run");
                                        return;
                                    }
                                }
                            } else {
                                return;
                            }
                        }
                    };
                }
            })
            .expect("spawn integration test worker thread")
    }

    fn run_spec(&self, task: &ScheduledSpec, retried: usize, started_at: Instant) {
        let spec = task.spec.as_ref();
        let outbox = self.outbox.clone();
        let spec_dir = match TempPathBuf::new_spec_dir(&task.dir_label()) {
            Ok(dir) => dir,
            Err(err) => {
                let finished_at = Instant::now();
                let duration = finished_at.saturating_duration_since(started_at);
                let msg = format!(
                    "failed to create spec tempdir for {}: {err}",
                    task.display_name
                );
                let _ = outbox.send(Notify::Error {
                    spec_error: Box::new(msg),
                    spec_name: task.display_name.clone(),
                    base_name: task.base_name.clone(),
                    duration,
                    finished_at,
                    spec_dir: None,
                    report_path: None,
                    runner_log_path: None,
                    node_log_paths: Vec::new(),
                    node_paths: Vec::new(),
                });
                return;
            }
        };
        let spec_dir_path = spec_dir.path().to_path_buf();
        if retried == 0 {
            let _ = outbox.send(Notify::Start {
                spec_name: task.display_name.clone(),
                base_name: task.base_name.clone(),
                spec_dir: spec_dir_path.clone(),
                started_at,
            });
        }

        let _dir_guard = SpecRunDirGuard::enter(spec_dir_path.clone());
        let _log_guard = SpecLogGuard::enter(&spec_dir_path.join("runner.log"));

        let setup_result = panic::catch_unwind(panic::AssertUnwindSafe(|| spec.before_run()));
        let mut nodes = match setup_result {
            Ok(nodes) => nodes,
            Err(spec_error) => {
                if retried < spec.setup().retry_failed {
                    error!(
                        "{} setup failed at {} attempt, retry...",
                        task.display_name, retried
                    );
                    drop(_log_guard);
                    drop(_dir_guard);
                    drop(spec_dir);
                    self.run_spec(task, retried + 1, started_at);
                } else {
                    let finished_at = Instant::now();
                    let duration = finished_at.saturating_duration_since(started_at);
                    drop(_log_guard);
                    let runner_log_path = read_runner_log_path(&spec_dir_path);
                    let node_log_paths = discover_spec_log_paths(&spec_dir_path, &[]);
                    let reason = extract_panic_message(spec_error.as_ref());
                    let report_path = write_failure_report(
                        &spec_dir_path,
                        task,
                        "SETUP_FAILED",
                        duration,
                        &reason,
                        &[],
                        runner_log_path.as_deref(),
                        &node_log_paths,
                        &self.config,
                    );
                    drop(_dir_guard);
                    let _ = outbox.send(Notify::Error {
                        spec_error,
                        spec_name: task.display_name.clone(),
                        base_name: task.base_name.clone(),
                        duration,
                        finished_at,
                        spec_dir: Some(spec_dir),
                        report_path,
                        runner_log_path,
                        node_log_paths,
                        node_paths: Vec::new(),
                    });
                }
                return;
            }
        };

        // Used to extend the lifecycle of standalone TempPathBufs (if any were created outside spec_dir)
        let node_paths = nodes
            .iter()
            .map(|node| node.owned_working_dir())
            .collect::<Vec<_>>();
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            spec.run(&mut nodes);
        }));

        let explicit_logs: Vec<PathBuf> = nodes.iter().map(|node| node.log_path()).collect();
        let node_log_paths = discover_spec_log_paths(&spec_dir_path, &explicit_logs);

        // error handles
        let spec_error = result.err();
        let panicked_error = nodes_panicked(&node_log_paths);
        if (panicked_error || spec_error.is_some()) && retried < spec.setup().retry_failed {
            error!(
                "{} failed at {} attempt, retry...",
                task.display_name, retried
            );
            drop(nodes);
            drop(_log_guard);
            drop(_dir_guard);
            drop(spec_dir);
            self.run_spec(task, retried + 1, started_at);
        } else if panicked_error || spec_error.is_some() {
            let finished_at = Instant::now();
            let duration = finished_at.saturating_duration_since(started_at);
            drop(_log_guard);
            let runner_log_path = read_runner_log_path(&spec_dir_path);
            let status_label = if panicked_error { "PANICKED" } else { "FAILED" };
            let reason = spec_error
                .as_deref()
                .map(extract_panic_message)
                .unwrap_or_else(|| "ckb node process panicked (check node log)".to_string());

            // Capture live node RPC snapshots BEFORE stopping the child processes.
            let report_path = write_failure_report(
                &spec_dir_path,
                task,
                status_label,
                duration,
                &reason,
                &nodes,
                runner_log_path.as_deref(),
                &node_log_paths,
                &self.config,
            );

            if self.config.pause_on_fail {
                pause_on_failure(
                    task,
                    &reason,
                    &spec_dir_path,
                    report_path.as_deref(),
                    &nodes,
                );
            }

            // Stop child processes cleanly before handing paths to the coordinator.
            drop(nodes);
            drop(_dir_guard);

            if panicked_error {
                let _ = outbox.send(Notify::Panick {
                    spec_name: task.display_name.clone(),
                    base_name: task.base_name.clone(),
                    duration,
                    finished_at,
                    spec_dir,
                    report_path,
                    runner_log_path,
                    node_log_paths,
                    node_paths,
                });
            } else {
                let _ = outbox.send(Notify::Error {
                    spec_error: spec_error.unwrap(),
                    spec_name: task.display_name.clone(),
                    base_name: task.base_name.clone(),
                    duration,
                    finished_at,
                    spec_dir: Some(spec_dir),
                    report_path,
                    runner_log_path,
                    node_log_paths,
                    node_paths,
                });
            }
        } else {
            let finished_at = Instant::now();
            let duration = finished_at.saturating_duration_since(started_at);
            // Stop child processes cleanly BEFORE coordinator deletes `spec_dir`.
            drop(nodes);
            drop(_log_guard);
            drop(_dir_guard);
            let _ = outbox.send(Notify::Done {
                spec_name: task.display_name.clone(),
                base_name: task.base_name.clone(),
                duration,
                finished_at,
                spec_dir,
                node_paths,
            });
        }
    }
}

pub(crate) fn discover_spec_log_paths(spec_dir: &Path, existing: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut result = Vec::new();
    for path in existing {
        if seen.insert(path.clone()) {
            result.push(path.clone());
        }
    }
    if let Ok(entries) = fs::read_dir(spec_dir) {
        let mut child_dirs: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.is_dir())
            .collect();
        child_dirs.sort();
        for child_dir in child_dirs {
            let log_path = child_dir.join("data/logs/run.log");
            if log_path.exists() && seen.insert(log_path.clone()) {
                result.push(log_path);
            }
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn write_failure_report(
    spec_dir: &Path,
    task: &ScheduledSpec,
    status_label: &str,
    duration: Duration,
    reason: &str,
    nodes: &[Node],
    runner_log_path: Option<&Path>,
    node_log_paths: &[PathBuf],
    config: &WorkerConfig,
) -> Option<PathBuf> {
    let report_path = spec_dir.join("failure-report.txt");
    let mut out = String::new();
    let _ = writeln!(out, "=== CKB Integration Test Failure Report ===");
    let _ = writeln!(out, "Spec:     {}", task.display_name);
    let _ = writeln!(out, "Status:   {status_label}");
    let _ = writeln!(out, "Duration: {}", format_duration(duration));
    let _ = writeln!(out, "Seed:     {}", config.seed);
    let _ = writeln!(out, "Dir:      {}", spec_dir.display());
    if let Some(runner_log) = runner_log_path {
        let _ = writeln!(out, "Runner:   {}", runner_log.display());
    }
    let _ = writeln!(
        out,
        "Repro:    {} --seed {} --verbose --keep-tmp-data {}",
        config.rerun_cmd_prefix, config.seed, task.base_name
    );
    let _ = writeln!(out, "\n--- Failure Reason ---\n{reason}");

    if let Some(runner_log) = runner_log_path
        && let Ok(content) = fs::read_to_string(runner_log)
        && !content.is_empty()
    {
        let lines: Vec<&str> = content.lines().collect();
        let tail_start = lines.len().saturating_sub(120);
        let _ = writeln!(
            out,
            "\n--- Spec Runner Log (last {} of {} lines) ---",
            lines.len() - tail_start,
            lines.len()
        );
        for (offset, line) in lines[tail_start..].iter().enumerate() {
            let _ = writeln!(out, "  L{}: {line}", tail_start + offset + 1);
        }
    }

    if !nodes.is_empty() {
        let _ = writeln!(out, "\n--- Live Node Snapshots ({}) ---", nodes.len());
        for node in nodes {
            out.push_str(&node.diagnostic_snapshot());
            out.push('\n');
        }
    }

    if !node_log_paths.is_empty() {
        let _ = writeln!(out, "--- Node Log Excerpts ({}) ---", node_log_paths.len());
        for log_path in node_log_paths {
            let _ = writeln!(out, "\n[{}]", log_path.display());
            match fs::read_to_string(log_path) {
                Ok(content) => {
                    let lines: Vec<&str> = content.lines().collect();
                    let panic_lines: Vec<(usize, &str)> = lines
                        .iter()
                        .enumerate()
                        .filter(|(_, line)| line.contains("panicked at"))
                        .map(|(idx, line)| (idx + 1, *line))
                        .collect();
                    if !panic_lines.is_empty() {
                        let _ = writeln!(out, "  Panic lines:");
                        for (ln, line) in panic_lines.iter().take(10) {
                            let _ = writeln!(out, "    L{ln}: {line}");
                        }
                    }

                    let error_warn: Vec<(usize, &str)> = lines
                        .iter()
                        .enumerate()
                        .filter(|(_, line)| line.contains(" ERROR ") || line.contains(" WARN "))
                        .map(|(idx, line)| (idx + 1, *line))
                        .collect();
                    if !error_warn.is_empty() {
                        let start = error_warn.len().saturating_sub(30);
                        let _ = writeln!(
                            out,
                            "  Last {} ERROR/WARN lines (of {}):",
                            error_warn.len() - start,
                            error_warn.len()
                        );
                        for (ln, line) in &error_warn[start..] {
                            let _ = writeln!(out, "    L{ln}: {line}");
                        }
                    }

                    let tail_start = lines.len().saturating_sub(80);
                    let _ = writeln!(
                        out,
                        "  Tail (last {} of {} lines):",
                        lines.len() - tail_start,
                        lines.len()
                    );
                    for (offset, line) in lines[tail_start..].iter().enumerate() {
                        let _ = writeln!(out, "    L{}: {line}", tail_start + offset + 1);
                    }
                }
                Err(err) => {
                    let _ = writeln!(out, "  <unable to read log: {err}>");
                }
            }
        }
    }

    match fs::write(&report_path, out) {
        Ok(()) => Some(report_path),
        Err(err) => {
            error!(
                "failed to write failure report {}: {}",
                report_path.display(),
                err
            );
            None
        }
    }
}

fn pause_on_failure(
    task: &ScheduledSpec,
    reason: &str,
    spec_dir: &Path,
    report_path: Option<&Path>,
    nodes: &[Node],
) {
    let _guard = pause_mutex().lock();
    let mut stderr = io::stderr().lock();
    let _ = writeln!(
        stderr,
        "\n==================== PAUSED ON FAILURE (--pause-on-fail) ===================="
    );
    let _ = writeln!(stderr, "Spec:   {}", task.display_name);
    let _ = writeln!(stderr, "Reason: {reason}");
    let _ = writeln!(stderr, "Dir:    {}", spec_dir.display());
    if let Some(report) = report_path {
        let _ = writeln!(stderr, "Report: {}", report.display());
    }
    let _ = writeln!(stderr, "Live nodes ({}):", nodes.len());
    for node in nodes {
        let _ = writeln!(
            stderr,
            "  - {}  alive={}  rpc={}  log={}",
            node.spec_node_name(),
            node.is_alive(),
            node.rpc_listen(),
            node.log_path().display()
        );
    }
    if let Some(first) = nodes.first() {
        let _ = writeln!(
            stderr,
            "Example RPC: curl -s {} -H 'content-type: application/json' -d '{{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"get_tip_header\",\"params\":[]}}'",
            first.rpc_listen()
        );
    }
    let _ = write!(stderr, "Press Enter to stop nodes and continue... ");
    let _ = stderr.flush();
    drop(stderr);

    let mut buf = String::new();
    let _ = io::stdin().lock().read_line(&mut buf);
}

/// A group of workers
pub struct Workers {
    workers: Vec<(Sender<Command>, Worker)>,
    join_handles: Option<Vec<JoinHandle<()>>>,
    is_shutdown: bool,
}

impl Workers {
    /// Create n workers
    pub fn new(
        count: usize,
        tasks: Arc<Mutex<Vec<ScheduledSpec>>>,
        outbox: Sender<Notify>,
        config: WorkerConfig,
    ) -> Self {
        let mut sequential = Vec::new();
        {
            let mut all_tasks = tasks.lock();
            let mut parallel = Vec::with_capacity(all_tasks.len());
            for task in all_tasks.drain(..) {
                if SEQUENCIAL_TASKS.contains(&task.base_name.as_str()) {
                    debug!("schedule {} as sequential task", task.display_name);
                    sequential.push(task);
                } else {
                    parallel.push(task);
                }
            }
            *all_tasks = parallel;
        }
        let sequencial_tasks = Arc::new(Mutex::new(sequential));
        let workers: Vec<_> = (0..count)
            .map({
                let tasks = Arc::clone(&tasks);
                let sequencial_tasks = Arc::clone(&sequencial_tasks);
                move |id| {
                    let (command_tx, command_rx) = unbounded();
                    let worker = Worker::new(
                        id,
                        Arc::clone(&tasks),
                        Arc::clone(&sequencial_tasks),
                        command_rx,
                        outbox.clone(),
                        config.clone(),
                    );
                    (command_tx, worker)
                }
            })
            .collect();
        Workers {
            workers,
            join_handles: None,
            is_shutdown: false,
        }
    }

    pub fn has_sequential_tasks(&self) -> bool {
        self.workers
            .first()
            .is_some_and(|(_, worker)| !worker.sequencial_tasks.lock().is_empty())
    }

    /// start all workers
    pub fn start(&mut self) {
        self.workers.first_mut().unwrap().1.sequencial_worker = true;

        let mut join_handles = Vec::new();
        for w in self.workers.iter_mut() {
            let h = w.1.clone().start();
            join_handles.push(h);
        }
        self.join_handles.replace(join_handles);
    }

    pub fn start_sequencial(&mut self) {
        if let Err(err) = self
            .workers
            .first()
            .unwrap()
            .0
            .send(Command::StartSequencial)
        {
            error!("start sequential worker failed, error: {}", err);
        } else {
            debug!("start sequential worker success")
        }
    }

    /// shutdown all workers, must call join_all after this.
    pub fn shutdown(&mut self) {
        if self.is_shutdown {
            return;
        }
        for w in &self.workers {
            if let Err(err) = w.0.send(Command::Shutdown) {
                info!("shutdown worker failed, error: {}", err);
            }
        }
        self.is_shutdown = true;
    }

    /// wait all workers to shutdown
    pub fn join_all(&mut self) {
        if self.join_handles.is_none() {
            return;
        }
        // make sure shutdown all workers
        self.shutdown();
        for h in self.join_handles.take().unwrap() {
            h.join().expect("wait worker shutdown");
        }
    }
}
