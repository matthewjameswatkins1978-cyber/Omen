use omen_core::{CoreError, ProcessExit, PtySessionId};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const DEFAULT_RING_BUFFER_CAPACITY: usize = 65536; // 64 KiB

/// In-memory bounded ring buffer for PTY output streams.
#[derive(Debug, Clone)]
pub struct RingBuffer {
    capacity: usize,
    buffer: Vec<u8>,
    total_written: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            buffer: Vec::with_capacity(capacity.min(4096)),
            total_written: 0,
        }
    }

    pub fn write(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
        self.total_written += data.len();
        if self.buffer.len() > self.capacity {
            let excess = self.buffer.len() - self.capacity;
            self.buffer.drain(..excess);
        }
    }

    pub fn read_all(&self) -> Vec<u8> {
        self.buffer.clone()
    }

    pub fn read_from(&self, offset: usize) -> (Vec<u8>, usize) {
        if offset >= self.total_written {
            return (Vec::new(), self.total_written);
        }
        let oldest_available_offset = self.total_written.saturating_sub(self.buffer.len());
        if offset <= oldest_available_offset {
            (self.buffer.clone(), self.total_written)
        } else {
            let relative_start = offset - oldest_available_offset;
            (self.buffer[relative_start..].to_vec(), self.total_written)
        }
    }

    pub fn total_written(&self) -> usize {
        self.total_written
    }
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_RING_BUFFER_CAPACITY)
    }
}

/// Sanitizes raw terminal byte output by stripping ANSI escape sequences,
/// control characters, and OSC strings before storing into logs, CAS, or agent context.
pub fn sanitize_terminal_escapes(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == 0x1b {
            // ESC sequence
            i += 1;
            if i < raw.len() {
                match raw[i] {
                    b'[' => {
                        // CSI sequence: ESC [ ... [final byte 0x40..=0x7E]
                        i += 1;
                        while i < raw.len() && !(0x40..=0x7E).contains(&raw[i]) {
                            i += 1;
                        }
                        if i < raw.len() {
                            i += 1;
                        }
                    }
                    b']' => {
                        // OSC sequence: ESC ] ... BEL (0x07) or ST (ESC \)
                        i += 1;
                        while i < raw.len() {
                            if raw[i] == 0x07 {
                                i += 1;
                                break;
                            }
                            if raw[i] == 0x1b && i + 1 < raw.len() && raw[i + 1] == b'\\' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    _ => {
                        i += 1;
                    }
                }
            }
        } else if raw[i] == b'\r' {
            i += 1;
        } else if raw[i] == b'\n' || raw[i] == b'\t' || raw[i] >= 0x20 {
            out.push(raw[i] as char);
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

pub struct NativePtyHandle {
    pub session_id: PtySessionId,
    pub pid: Option<u32>,
    pub child: Option<tokio::process::Child>,
    pub input_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    pub ring_buffer: Arc<Mutex<RingBuffer>>,
    #[cfg(windows)]
    pub job_guard: Option<crate::platform::windows::JobObjectGuard>,
    pub rows: u16,
    pub cols: u16,
}

impl NativePtyHandle {
    pub fn new(
        session_id: PtySessionId,
        mut child: tokio::process::Child,
        #[cfg(windows)] job_guard: Option<crate::platform::windows::JobObjectGuard>,
        rows: u16,
        cols: u16,
    ) -> Self {
        let pid = child.id();
        let ring_buffer = Arc::new(Mutex::new(RingBuffer::default()));

        // Set up input forwarding channel
        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        if let Some(mut stdin) = child.stdin.take() {
            tokio::spawn(async move {
                while let Some(bytes) = input_rx.recv().await {
                    if stdin.write_all(&bytes).await.is_err() {
                        break;
                    }
                    let _ = stdin.flush().await;
                }
            });
        }

        // Set up stdout reading task
        if let Some(mut stdout) = child.stdout.take() {
            let rb = ring_buffer.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stdout.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    rb.lock().unwrap().write(&buf[..n]);
                }
            });
        }

        Self {
            session_id,
            pid,
            child: Some(child),
            input_tx: Some(input_tx),
            ring_buffer,
            #[cfg(windows)]
            job_guard,
            rows,
            cols,
        }
    }
}

impl crate::backend::PtyExecutionHandle for NativePtyHandle {
    fn session_id(&self) -> PtySessionId {
        self.session_id.clone()
    }

    fn pid(&self) -> Option<u32> {
        self.pid
    }

    fn write_input(&mut self, data: &[u8]) -> Result<(), CoreError> {
        if let Some(tx) = &self.input_tx {
            tx.send(data.to_vec()).map_err(|e| {
                CoreError::ExecutionFailed(format!("Failed to write PTY input: {e}"))
            })?;
            Ok(())
        } else {
            Err(CoreError::ExecutionFailed("PTY stdin closed".into()))
        }
    }

    fn read_output(&mut self) -> Result<Vec<u8>, CoreError> {
        Ok(self.ring_buffer.lock().unwrap().read_all())
    }

    fn resize(&mut self, rows: u16, cols: u16) -> Result<(), CoreError> {
        self.rows = rows;
        self.cols = cols;
        Ok(())
    }

    fn terminate(&mut self) -> Result<(), CoreError> {
        self.input_tx.take(); // close input
        #[cfg(windows)]
        if let Some(jg) = &self.job_guard {
            jg.terminate(1);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
        Ok(())
    }

    fn try_wait(&mut self) -> Result<Option<ProcessExit>, CoreError> {
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => Ok(Some(ProcessExit {
                    code: status.code(),
                    signal: None,
                })),
                Ok(None) => Ok(None),
                Err(e) => Err(CoreError::ExecutionFailed(format!(
                    "Failed to poll PTY child: {e}"
                ))),
            }
        } else {
            Ok(Some(ProcessExit {
                code: Some(1),
                signal: Some("TERMINATED".into()),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_bounded_retention() {
        let mut rb = RingBuffer::new(10);
        rb.write(b"abcdefghij"); // 10 bytes
        assert_eq!(rb.read_all(), b"abcdefghij");
        assert_eq!(rb.total_written(), 10);

        rb.write(b"12345"); // 5 more bytes -> exceeds capacity
        assert_eq!(rb.read_all(), b"fghij12345");
        assert_eq!(rb.total_written(), 15);

        let (from_10, total) = rb.read_from(10);
        assert_eq!(total, 15);
        assert_eq!(from_10, b"12345");
    }

    #[test]
    fn test_escape_sanitization() {
        let input = b"\x1b[31;1mError:\x1b[0m File not found\x1b]0;Title\x07\r\nDone";
        let clean = sanitize_terminal_escapes(input);
        assert_eq!(clean, "Error: File not found\nDone");
    }
}
