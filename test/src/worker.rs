use crate::{utils::nodes_panicked, Spec};
use ckb_channel::{unbounded, Receiver, Sender};
use ckb_util::Mutex;
use log::{error, info};
use std::any::Any;
use std::panic;
use std::path::PathBuf;
use std::sync::{atomic::AtomicU16, Arc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

/// Commands
#[derive(PartialEq)]
pub enum Command {
    Shutdown,
}

/// Notify from worker
pub enum Notify {
    Start {
        spec_name: String,
    },
    Done {
        spec_name: String,
        seconds: u64,
    },
    Error {
        spec_error: Box<dyn Any + Send>,
        spec_name: String,
        node_log_paths: Vec<PathBuf>,
    },
    Panick {
        spec_name: String,
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
}

impl Clone for Worker {
    fn clone(&self) -> Self {
        Self {
            tasks: Arc::clone(&self.tasks),
            inbox: self.inbox.clone(),
            outbox: self.outbox.clone(),
            start_port: Arc::clone(&self.start_port),
        }
    }
}

impl Worker {
    pub fn new(
        tasks: Arc<Mutex<Vec<Box<dyn Spec>>>>,
        inbox: Receiver<Command>,
        outbox: Sender<Notify>,
        start_port: Arc<AtomicU16>,
    ) -> Self {
        Worker {
            tasks,
            inbox,
            outbox,
            start_port,
        }
    }

    /// start handle tasks
    pub fn start(self) -> JoinHandle<()> {
        thread::spawn(move || {
            loop {
                let msg = match self.inbox.try_recv() {
                    Ok(msg) => Some(msg),
                    Err(err) => {
                        if !err.is_empty() {
                            self.outbox.send(Notify::Stop).unwrap();
                            panic!(err)
                        }
                        None
                    }
                };
                // check command
                if Some(Command::Shutdown) == msg {
                    self.outbox.send(Notify::Stop).unwrap();
                    return;
                }
                // pick a spec to run
                let spec = match self.tasks.lock().pop() {
                    Some(spec) => spec,
                    None => {
                        self.outbox.send(Notify::Stop).unwrap();
                        return;
                    }
                };

                self.run_spec(spec.as_ref(), 0);
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
                    node_log_paths,
                })
                .unwrap();
        } else if let Some(spec_error) = spec_error {
            outbox
                .send(Notify::Error {
                    spec_error,
                    spec_name: spec.name().to_string(),
                    node_log_paths,
                })
                .unwrap();
        } else {
            outbox
                .send(Notify::Done {
                    spec_name: spec.name().to_string(),
                    seconds: now.elapsed().as_secs(),
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
        let workers: Vec<_> = (0..count)
            .map({
                let tasks = Arc::clone(&tasks);
                move |_| {
                    let (command_tx, command_rx) = unbounded();
                    let worker = Worker::new(
                        Arc::clone(&tasks),
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
        let mut join_handles = Vec::new();
        for w in self.workers.iter_mut() {
            let h = w.1.clone().start();
            join_handles.push(h);
        }
        self.join_handles.replace(join_handles);
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
