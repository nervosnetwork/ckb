use crate::node::Node;
use crate::utils::{adaptive_poll_interval, tweaked_duration};
use std::fs::{File, metadata};
use std::io::{BufRead, BufReader, ErrorKind, Seek, SeekFrom};
use std::thread;
use std::time::Instant;

pub fn monitor_log_until_expected_show(
    node: &Node,
    seek_from: u64,
    timeout: u64,
    expected: &str,
) -> Option<String> {
    let predicate = |line: &str| {
        if line.contains(expected) {
            Some(line.to_owned())
        } else {
            None
        }
    };
    monitor_log_until(node, seek_from, timeout, predicate)
}

pub fn monitor_log_until_expected_count(
    node: &Node,
    seek_from: u64,
    timeout: u64,
    expected: &str,
    min_count: usize,
) -> bool {
    if min_count == 0 {
        return true;
    }
    let mut count = 0usize;
    monitor_log_until(node, seek_from, timeout, |line| {
        if line.contains(expected) {
            count += 1;
            if count >= min_count {
                return Some(());
            }
        }
        None
    })
    .is_some()
}

fn monitor_log_until<T, P>(
    node: &Node,
    mut seek_from: u64,
    timeout: u64,
    mut predicate: P,
) -> Option<T>
where
    P: FnMut(&str) -> Option<T>,
{
    let deadline = Instant::now() + tweaked_duration(timeout);
    let filepath = node.log_path();
    let mut file: Option<File> = None;
    let mut attempt = 0u32;
    let mut line_buf = String::new();
    loop {
        if file.is_none() {
            match File::open(&filepath) {
                Ok(opened) => file = Some(opened),
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => panic!("failed to open node log {}: {err}", filepath.display()),
            }
        }

        if let Some(ref mut opened) = file {
            let file_size = metadata(&filepath).map(|m| m.len()).unwrap_or(seek_from);
            if file_size > seek_from {
                opened.seek(SeekFrom::Start(seek_from)).unwrap();
                let mut reader = BufReader::new(&*opened);
                loop {
                    line_buf.clear();
                    let bytes_read = reader.read_line(&mut line_buf).unwrap();
                    if bytes_read == 0 {
                        break;
                    }
                    // Wait for a full line before advancing `seek_from` to avoid skipping
                    // a partially flushed line.
                    if !line_buf.ends_with('\n') {
                        break;
                    }
                    seek_from += bytes_read as u64;
                    let trimmed = line_buf.trim_end_matches(['\r', '\n']);
                    if let Some(result) = predicate(trimmed) {
                        return Some(result);
                    }
                }
            }
        }

        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let sleep_dur = adaptive_poll_interval(attempt, 10, 100).min(deadline - now);
        thread::sleep(sleep_dur);
        attempt = attempt.saturating_add(1);
    }
    None
}
