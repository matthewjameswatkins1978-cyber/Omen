use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Debug)]
pub enum PlatformStream {
    #[cfg(windows)]
    NamedPipe(tokio::net::windows::named_pipe::NamedPipeClient),
    #[cfg(windows)]
    NamedPipeServer(tokio::net::windows::named_pipe::NamedPipeServer),
    #[cfg(not(windows))]
    Unix(tokio::net::UnixStream),
    Duplex(tokio::io::DuplexStream),
}

impl PlatformStream {
    pub fn duplex_pair(capacity: usize) -> (Self, Self) {
        let (a, b) = tokio::io::duplex(capacity);
        (PlatformStream::Duplex(a), PlatformStream::Duplex(b))
    }

    pub async fn connect(endpoint: &str) -> io::Result<Self> {
        #[cfg(windows)]
        {
            use std::time::{Duration, Instant};
            use tokio::net::windows::named_pipe::ClientOptions;

            let start = Instant::now();
            let timeout = Duration::from_secs(5);
            loop {
                match ClientOptions::new().open(endpoint) {
                    Ok(client) => return Ok(PlatformStream::NamedPipe(client)),
                    Err(e) if e.raw_os_error() == Some(231) /* ERROR_PIPE_BUSY */ => {
                        if start.elapsed() > timeout {
                            return Err(e);
                        }
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        #[cfg(not(windows))]
        {
            let stream = tokio::net::UnixStream::connect(endpoint).await?;
            Ok(PlatformStream::Unix(stream))
        }
    }
}

impl AsyncRead for PlatformStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut *self {
            #[cfg(windows)]
            PlatformStream::NamedPipe(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(windows)]
            PlatformStream::NamedPipeServer(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(not(windows))]
            PlatformStream::Unix(s) => Pin::new(s).poll_read(cx, buf),
            PlatformStream::Duplex(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for PlatformStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut *self {
            #[cfg(windows)]
            PlatformStream::NamedPipe(s) => Pin::new(s).poll_write(cx, buf),
            #[cfg(windows)]
            PlatformStream::NamedPipeServer(s) => Pin::new(s).poll_write(cx, buf),
            #[cfg(not(windows))]
            PlatformStream::Unix(s) => Pin::new(s).poll_write(cx, buf),
            PlatformStream::Duplex(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            #[cfg(windows)]
            PlatformStream::NamedPipe(s) => Pin::new(s).poll_flush(cx),
            #[cfg(windows)]
            PlatformStream::NamedPipeServer(s) => Pin::new(s).poll_flush(cx),
            #[cfg(not(windows))]
            PlatformStream::Unix(s) => Pin::new(s).poll_flush(cx),
            PlatformStream::Duplex(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            #[cfg(windows)]
            PlatformStream::NamedPipe(s) => Pin::new(s).poll_shutdown(cx),
            #[cfg(windows)]
            PlatformStream::NamedPipeServer(s) => Pin::new(s).poll_shutdown(cx),
            #[cfg(not(windows))]
            PlatformStream::Unix(s) => Pin::new(s).poll_shutdown(cx),
            PlatformStream::Duplex(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

pub struct PlatformListener {
    endpoint: String,
    #[cfg(windows)]
    pipe_server: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    #[cfg(not(windows))]
    unix_listener: Option<tokio::net::UnixListener>,
}

impl PlatformListener {
    pub async fn bind(endpoint: &str) -> io::Result<Self> {
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(endpoint)?;
            Ok(Self {
                endpoint: endpoint.to_string(),
                pipe_server: Some(server),
            })
        }

        #[cfg(not(windows))]
        {
            use std::path::Path;
            let path = Path::new(endpoint);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
                }
            }
            if path.exists() {
                if tokio::net::UnixStream::connect(path).await.is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "Another daemon instance is already listening on this socket",
                    ));
                }
                let _ = std::fs::remove_file(path);
            }
            let listener = tokio::net::UnixListener::bind(path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
            }
            Ok(Self {
                endpoint: endpoint.to_string(),
                unix_listener: Some(listener),
            })
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn accept(&mut self) -> io::Result<PlatformStream> {
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let current = self
                .pipe_server
                .take()
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "Pipe server missing"))?;
            current.connect().await?;

            let next = ServerOptions::new()
                .first_pipe_instance(false)
                .create(&self.endpoint)?;
            self.pipe_server = Some(next);

            Ok(PlatformStream::NamedPipeServer(current))
        }

        #[cfg(not(windows))]
        {
            let listener = self.unix_listener.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::BrokenPipe, "Unix listener missing")
            })?;
            let (stream, _) = listener.accept().await?;
            Ok(PlatformStream::Unix(stream))
        }
    }
}

#[cfg(not(windows))]
impl Drop for PlatformListener {
    fn drop(&mut self) {
        let path = std::path::Path::new(&self.endpoint);
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }
}
