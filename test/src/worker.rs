use crate::{utils::nodes_panicked, Spec};
use ckb_channel::{unbounded, Receiver, Sender};
use ckb_logger::{error, info};
use ckb_util::Mutex;
use std::any::Any;
use std::panic;
use std::path::PathBuf;
use std::sync::{atomic::AtomicU16, Arc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

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
    },
    Done {
        spec_name: String,
        seconds: u64,
        node_paths: Vec<PathBuf>,
    },
    Error {
        spec_error: Box<dyn Any + Send>,
        spec_name: String,
        seconds: u64,
        node_log_paths: Vec<PathBuf>,
    },
    Panick {
        spec_name: String,
        seconds: u64,
        node_log_paths: Vec<PathBuf>,
    },
    Stop,
}

/// Worker
pub struct Worker {
    tasks: Arc<Mutex<Vec<Box<dyn Spec>>>>,
    inbox: Receiver<Command>,
    outbox: Sender<Notify>,
    start_port: Arc<AtomicU16>,

    sequencial_tasks: Arc<Mutex<Vec<Box<dyn Spec>>>>,
    sequencial_worker: bool,
}

impl Clone for Worker {
    fn clone(&self) -> Self {
        Self {
            tasks: Arc::clone(&self.tasks),
            inbox: self.inbox.clone(),
            outbox: self.outbox.clone(),
            start_port: Arc::clone(&self.start_port),
            sequencial_tasks: Arc::clone(&self.sequencial_tasks),
            sequencial_worker: self.sequencial_worker,
        }
    }
}

const SEQUENCIAL_TASKS: &[&str] = &["RandomlyKill", "SyncChurn"];

impl Worker {
    pub fn new(
        tasks: Arc<Mutex<Vec<Box<dyn Spec>>>>,
        sequencial_tasks: Arc<Mutex<Vec<Box<dyn Spec>>>>,
        inbox: Receiver<Command>,
        outbox: Sender<Notify>,
        start_port: Arc<AtomicU16>,
    ) -> Self {
        Worker {
            tasks,
            inbox,
            outbox,
            start_port,
            sequencial_tasks,
            sequencial_worker: false,
        }
    }

    /// start handle tasks
    pub fn start(self) -> JoinHandle<()> {
        thread::spawn(move || {
            let mut start_sequencial_task = false;

            loop {
                let msg = match self.inbox.try_recv() {
                    Ok(msg) => Some(msg),
                    Err(err) => {
                        if !err.is_empty() {
                            self.outbox.send(Notify::Stop).unwrap();
                            std::panic::panic_any(err)
                        }
                        None
                    }
                };
                // check command
                match msg {
                    Some(Command::StartSequencial) => {
                        start_sequencial_task = true;
                    }
                    Some(Command::Shutdown) => {
                        self.outbox.send(Notify::Stop).unwrap();
                        return;
                    }
                    _ => {}
                }

                // pick a spec to run

                let task = self.tasks.lock().pop();
                match task {
                    Some(spec) => {
                        // if spec.name() is RandomlyKill or SyncChurn, then push it to sequencial_tasks
                        if SEQUENCIAL_TASKS.contains(&spec.name()) {
                            info!("push {} to sequencial_tasks", spec.name());
                            self.sequencial_tasks.lock().push(spec);
                        } else {
                            self.run_spec(spec.as_ref(), 0);
                        }
                    }
                    None => {
                        if self.sequencial_worker {
                            info!("sequencial worker is waiting for command");
                            if start_sequencial_task {
                                match self.sequencial_tasks.lock().pop() {
                                    Some(spec) => {
                                        self.run_spec(spec.as_ref(), 0);
                                    }
                                    None => {
                                        info!("sequencial worker has no task to run");
                                        self.outbox.send(Notify::Stop).unwrap();
                                        return;
                                    }
                                };
                            } else {
                                info!("sequencial worker is waiting for parallel workers finish");
                                std::thread::sleep(std::time::Duration::from_secs(1));
                            }
                        } else {
                            self.outbox.send(Notify::Stop).unwrap();
                            return;
                        }
                    }
                };
            }
        })
    }

    fn run_spec(&self, spec: &dyn Spec, retried: usize) {
        let outbox = self.outbox.clone();
        let now = Instant::now();
        outbox
            .send(Notify::Start {
                spec_name: spec.name().to_string(),
            })
            .unwrap();

        let mut nodes = spec.before_run();
        let node_paths = nodes
            .iter()
            .map(|node| node.working_dir())
            .collect::<Vec<_>>();
        let node_log_paths = nodes.iter().map(|node| node.log_path()).collect::<Vec<_>>();
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            spec.run(&mut nodes);
        }));

        // error handles
        let spec_error = result.err();
        let panicked_error = nodes_panicked(&node_log_paths);
        if (panicked_error || spec_error.is_some()) && retried < spec.setup().retry_failed {
            error!("{} failed at {} attempt, retry...", spec.name(), retried);
            self.run_spec(spec, retried + 1);
        } else if panicked_error {
            outbox
                .send(Notify::Panick {
                    spec_name: spec.name().to_string(),
                    seconds: now.elapsed().as_secs(),
                    node_log_paths,
                })
                .unwrap();
        } else if let Some(spec_error) = spec_error {
            outbox
                .send(Notify::Error {
                    spec_error,
                    spec_name: spec.name().to_string(),
                    seconds: now.elapsed().as_secs(),
                    node_log_paths,
                })
                .unwrap();
        } else {
            outbox
                .send(Notify::Done {
                    spec_name: spec.name().to_string(),
                    seconds: now.elapsed().as_secs(),
                    node_paths,
                })
                .unwrap();
        }
    }
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
        tasks: Arc<Mutex<Vec<Box<dyn Spec>>>>,
        outbox: Sender<Notify>,
        start_port: u16,
    ) -> Self {
        let start_port = Arc::new(AtomicU16::new(start_port));

        let sequencial_tasks = Arc::new(Mutex::new(Vec::new()));
        let workers: Vec<_> = (0..count)
            .map({
                let tasks = Arc::clone(&tasks);
                let sequencial_tasks = Arc::clone(&sequencial_tasks);
                move |_| {
                    let (command_tx, command_rx) = unbounded();
                    let worker = Worker::new(
                        Arc::clone(&tasks),
                        Arc::clone(&sequencial_tasks),
                        command_rx,
                        outbox.clone(),
                        Arc::clone(&start_port),
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
            error!("start sequencial worker failed, error: {}", err);
        } else {
            info!("start sequencial worker success")
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
