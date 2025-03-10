use std::io::{Error, ErrorKind, Read, Write};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

/// A bidirectional communication channel for transferring data between native code and CKB-VM.
///
/// `Pipe` implements a buffered channel that can be used for either reading or writing, but not both
/// simultaneously. It uses a synchronous channel internally to ensure proper flow control.
///
/// # Structure
/// * `tx` - Optional sender end of the channel
/// * `rx` - Optional receiver end of the channel, wrapped in a mutex for thread safety
/// * `buf` - Internal buffer for storing partially read data
///
/// # Examples
/// ```ignore
/// // Create a pipe pair for bidirectional communication
/// let (pipe1, pipe2) = Pipe::new_pair();
///
/// // Write to pipe2
/// pipe2.write(b"hello")?;
///
/// // Read from pipe1
/// let mut buf = vec![0; 5];
/// let n = pipe1.read(&mut buf)?;
/// assert_eq!(&buf[..n], b"hello");
/// ```
///
/// # Implementation Details
/// - The pipe uses a zero-capacity channel (`sync_channel(0)`), making all write operations
///   synchronous
/// - Reading is buffered: data is read from the channel into an internal buffer and then
///   served from there
/// - Either `tx` or `rx` will be `Some`, but never both, determining whether the pipe is
///   for reading or writing
///
/// # Thread Safety
/// - The receiver is wrapped in a `Mutex` to ensure thread-safe access
/// - The sender is naturally thread-safe through `SyncSender`
///
/// # Resource Management
/// The pipe automatically closes when dropped, ensuring proper cleanup of system resources.
pub struct Pipe {
    tx: Option<SyncSender<Vec<u8>>>,
    rx: Option<Mutex<Receiver<Vec<u8>>>>,
    buf: Vec<u8>,
}

impl Pipe {
    /// Creates a new pair of pipes for bidirectional communication.
    pub fn new_pair() -> (Self, Self) {
        let (tx, rx) = sync_channel(0);
        (
            Self {
                tx: None,
                rx: Some(Mutex::new(rx)),
                buf: vec![],
            },
            Self {
                tx: Some(tx),
                rx: None,
                buf: vec![],
            },
        )
    }

    /// Closes the pipe, ensuring proper cleanup of system resources.
    pub fn close(&mut self) {
        if self.tx.is_some() {
            drop(self.tx.take());
        }
        if self.rx.is_some() {
            drop(self.rx.take());
        }
    }
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        if self.buf.is_empty() {
            match self
                .rx
                .as_ref()
                .ok_or_else(|| Error::new(ErrorKind::Other, "channel is not found"))?
                .lock()
                .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?
                .recv()
            {
                Ok(data) => self.buf = data,
                Err(e) => {
                    return Err(Error::new(ErrorKind::Other, e.to_string()));
                }
            }
        }
        let len = self.buf.len().min(buf.len());
        buf[..len].copy_from_slice(&self.buf[..len]);
        self.buf = self.buf.split_off(len);
        Ok(len)
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self
            .tx
            .as_mut()
            .ok_or_else(|| Error::new(ErrorKind::Other, "channel is not found"))?
            .send(buf.to_vec())
        {
            Ok(_) => Ok(buf.len()),
            Err(e) => Err(Error::new(ErrorKind::Other, e.to_string())),
        }
    }

    fn flush(&mut self) -> Result<(), Error> {
        Ok(())
    }
}
