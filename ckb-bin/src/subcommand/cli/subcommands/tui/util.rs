use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use termion::event::Key;
use termion::input::TermRead;

pub fn ts_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64
}

pub fn human_capacity(value: u64) -> String {
    let value_f64 = value as f64 / 10000.0 / 10000.0;
    if value_f64 >= (1024.0 * 1024.0) {
        format!("{:.2} MCKB", value_f64 / 1024.0 / 1024.0)
    } else if value_f64 >= 1024.0 {
        format!("{:.2} KCKB", value_f64 / 1024.0)
    } else {
        format!("{:.1} CKB", value_f64)
    }
}

pub struct App {
    pub(crate) menu_active: bool,
    pub(crate) tabs: TabsState,
}

pub struct TabsState {
    pub titles: Vec<String>,
    pub index: usize,
}

impl TabsState {
    pub fn new(titles: Vec<&str>) -> TabsState {
        let titles = titles.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        TabsState { titles, index: 0 }
    }
    pub fn fixed_titles(&self) -> Vec<String> {
        let max_length = self.titles.iter().map(String::len).max().unwrap_or(0) + 1;
        self.titles
            .iter()
            .map(|title| format!("{:^width$}", title, width = max_length))
            .collect::<Vec<_>>()
    }
    pub fn next(&mut self) {
        self.index = (self.index + 1) % self.titles.len();
    }

    pub fn previous(&mut self) {
        if self.index > 0 {
            self.index -= 1;
        } else {
            self.index = self.titles.len() - 1;
        }
    }
}

pub enum Event<I> {
    Input(I),
    Tick,
}

/// A small event handler that wrap termion input and tick events. Each event
/// type is handled in its own thread and returned to a common `Receiver`
pub struct Events {
    rx: mpsc::Receiver<Event<Key>>,
    _input_handle: thread::JoinHandle<()>,
    _tick_handle: thread::JoinHandle<()>,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub exit_key: Key,
    pub tick_rate: Duration,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            exit_key: Key::Char('q'),
            tick_rate: Duration::from_millis(250),
        }
    }
}

impl Events {
    pub fn new() -> Events {
        Events::with_config(Config::default())
    }

    pub fn with_config(config: Config) -> Events {
        let (tx, rx) = mpsc::channel();
        let input_handle = {
            let tx = tx.clone();
            thread::spawn(move || {
                let stdin = io::stdin();
                for evt in stdin.keys() {
                    if let Ok(key) = evt {
                        if tx.send(Event::Input(key)).is_err() {
                            return;
                        }
                        if key == config.exit_key {
                            return;
                        }
                    }
                }
            })
        };
        let tick_handle = {
            let tx = tx.clone();
            thread::spawn(move || {
                let tx = tx.clone();
                loop {
                    if tx.send(Event::Tick).is_err() {
                        return;
                    }
                    thread::sleep(config.tick_rate);
                }
            })
        };
        Events {
            rx,
            _input_handle: input_handle,
            _tick_handle: tick_handle,
        }
    }

    pub fn next(&self) -> Result<Event<Key>, mpsc::RecvError> {
        self.rx.recv()
    }
}
