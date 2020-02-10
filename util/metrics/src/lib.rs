use chrono::prelude::{DateTime, Local};
use crossbeam_channel::{bounded, Sender};
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::thread;

static mut SENDER: Option<Sender<String>> = None;

pub fn metric(class: &str, context: &str) {
    let dt: DateTime<Local> = Local::now();
    let timestamp = dt.format("%Y-%m-%d %H:%M:%S%.3f %Z").to_string();
    let json = json!({
        "ts": timestamp,
        "class": class,
        "context": context,
    })
    .to_string();

    // Send metrics in json to the writer thread via global channel
    unsafe {
        if let Some(ref sender) = SENDER {
            let _ = sender.send(json);
        }
    }
}

pub fn init(filepath: PathBuf) {
    let (sender, receiver) = bounded::<String>(1024);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&filepath)
        .unwrap_or_else(|_| {
            panic!(
                "Cannot write to metrics file given: {:?}",
                filepath.as_os_str()
            )
        });
    let _ = thread::Builder::new()
        .name("MetricWriter".to_owned())
        .spawn(move || {
            while let Ok(metric) = receiver.recv() {
                let _ = file.write_all(metric.as_bytes());
                let _ = file.write_all(b"\n");
            }
        });

    // Initialize the global channel
    unsafe {
        SENDER = Some(sender);
    }
}
