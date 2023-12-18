use rand::Rng;
use tokio::sync::{mpsc, oneshot};
use tokio::task;

type Job = Box<dyn FnOnce() + Send + 'static>;

struct Manager {
    workers: Vec<(
        task::JoinHandle<()>,
        mpsc::UnboundedSender<Job>,
        oneshot::Sender<()>,
        mpsc::UnboundedReceiver<()>,
    )>,
}

impl Manager {
    fn new() -> Self {
        let mut workers = Vec::new();
        for _ in 0..num_cpus::get() {
            let (job_sender, job_receiver) = mpsc::unbounded_channel::<Job>();
            let (exit_sender, exit_receiver) = oneshot::channel();
            let (res_sender, res_receiver) = mpsc::unbounded_channel::<()>();
            let handler =
                task::spawn(async { worker(job_receiver, exit_receiver, res_sender).await });
            workers.push((handler, job_sender, exit_sender, res_receiver));
        }
        Self { workers }
    }

    async fn send_job(&mut self, job: Job) {
        // pick a random worker from workers and send job to it
        let mut rng = rand::thread_rng();
        let idx = rng.gen_range(0..self.workers.len());
        let worker = self.workers.get_mut(idx).unwrap();
        worker.1.send(job).unwrap();
        let res = worker.3.recv().await;
        println!("res: {:?}", res);
    }
}

async fn worker(
    mut job_receiver: mpsc::UnboundedReceiver<Job>,
    mut exit_receiver: oneshot::Receiver<()>,
    res_sender: mpsc::UnboundedSender<()>,
) {
    loop {
        tokio::select! {
            job = job_receiver.recv() => {
                if let Some(job) = job {
                    job();
                    let _ = res_sender.send(());
                } else {
                    break;
                }
            }
            _ = &mut exit_receiver => {
                break;
            }
        }
    }
}
