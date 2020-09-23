use crate::node::Node;
use crate::utils::{node_log, sleep, tweaked_duration};

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::time::Instant;

pub fn monitor_log_until_expected_show(
    node: &Node,
    seek_from: u64,
    timeout: u64,
    expected: &str,
) -> Option<String> {
    let predicate = |file_reader: BufReader<&File>| {
        for line in file_reader.lines() {
            let line = line.unwrap();
            if line.contains(expected) {
                return Some(line);
            }
        }
        None
    };
    monitor_log_until(node, seek_from, timeout, predicate)
}

fn monitor_log_until<P>(
    node: &Node,
    mut seek_from: u64,
    timeout: u64,
    predicate: P,
) -> Option<String>
where
    P: Fn(BufReader<&File>) -> Option<String>,
{
    let timeout = tweaked_duration(timeout);
    let start = Instant::now();
    let filename = node_log(node.working_dir().to_str().unwrap());
    let mut file = File::open(&filename).unwrap();
    loop {
        let file_size = node.log_size();
        if seek_from != file_size {
            file.seek(SeekFrom::Start(seek_from)).unwrap();
            let file_reader = BufReader::new(&file);
            if let Some(result) = predicate(file_reader) {
                return Some(result);
            }
            seek_from = file_size;
        } else if start.elapsed() > timeout {
            break;
        } else {
            sleep(1)
        }
    }
    None
}
