use std::io::Read;
use std::io::Result as IoResult;
use std::sync::mpsc::channel;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use crate::limits::Leftover;

/// A `Reader` that reads exactly the number of bytes from a sub-reader.
///
/// If the limit is reached, it returns EOF. Holon (PATCHED.md): if the limit is not reached when
/// the destructor is called, the remaining bytes are left unread, and how many is told to the
/// connection (`telling`), which reads them off before its next request, within bounds
/// (`limits::Leftover`). Upstream read them off here, into a buffer as long as what was left.
pub struct EqualReader<R>
where
    R: Read,
{
    reader: R,
    size: usize,
    #[allow(dead_code)]
    last_read_signal: Sender<IoResult<()>>,
    left: Option<Arc<Leftover>>,
}

impl<R> EqualReader<R>
where
    R: Read,
{
    pub fn new(reader: R, size: usize) -> (EqualReader<R>, Receiver<IoResult<()>>) {
        let (tx, rx) = channel();

        let r = EqualReader {
            reader,
            size,
            last_read_signal: tx,
            left: None,
        };

        (r, rx)
    }

    /// Holon (PATCHED.md): told, as it is dropped, what of its bytes are left unread.
    pub(crate) fn telling(mut self, left: Arc<Leftover>) -> EqualReader<R> {
        self.left = Some(left);
        self
    }
}

impl<R> Read for EqualReader<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if self.size == 0 {
            return Ok(0);
        }

        let buf = if buf.len() < self.size {
            buf
        } else {
            &mut buf[..self.size]
        };

        match self.reader.read(buf) {
            Ok(len) => {
                self.size -= len;
                Ok(len)
            }
            err @ Err(_) => err,
        }
    }
}

impl<R> Drop for EqualReader<R>
where
    R: Read,
{
    // Holon (PATCHED.md): reads nothing; what is left is told to the connection
    fn drop(&mut self) {
        if let Some(left) = &self.left {
            left.bytes(self.size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EqualReader;
    use std::io::Read;

    #[test]
    fn test_limit() {
        use std::io::Cursor;

        let mut org_reader = Cursor::new("hello world".to_string().into_bytes());

        {
            let (mut equal_reader, _) = EqualReader::new(org_reader.by_ref(), 5);

            let mut string = String::new();
            equal_reader.read_to_string(&mut string).unwrap();
            assert_eq!(string, "hello");
        }

        let mut string = String::new();
        org_reader.read_to_string(&mut string).unwrap();
        assert_eq!(string, " world");
    }

    // Holon (PATCHED.md): dropped before its end, it reads nothing more and tells what is left
    // (upstream's read the rest off, and the reader after it read " world")
    #[test]
    fn test_not_enough() {
        use crate::limits::{Left, Leftover};
        use std::io::Cursor;
        use std::sync::Arc;

        let mut org_reader = Cursor::new("hello world".to_string().into_bytes());
        let left = Arc::new(Leftover::default());

        {
            let (equal_reader, _) = EqualReader::new(org_reader.by_ref(), 5);
            let mut equal_reader = equal_reader.telling(left.clone());

            let mut vec = [0];
            equal_reader.read_exact(&mut vec).unwrap();
            assert_eq!(vec[0], b'h');
        }

        assert_eq!(left.take(), Left::Bytes(4));
        let mut string = String::new();
        org_reader.read_to_string(&mut string).unwrap();
        assert_eq!(string, "ello world");
    }
}
