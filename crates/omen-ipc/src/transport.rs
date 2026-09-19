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

            Self::verify_windows_peer_admission(&current)?;

            Ok(PlatformStream::NamedPipeServer(current))
        }

        #[cfg(not(windows))]
        {
            let listener = self.unix_listener.as_ref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::BrokenPipe, "Unix listener missing")
            })?;
            let (stream, _) = listener.accept().await?;

            #[cfg(unix)]
            {
                let peer_cred = stream.peer_cred()?;
                let current_uid = rustix::process::getuid().as_raw();
                if peer_cred.uid() != current_uid {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!(
                            "Local peer admission denied: peer UID {} != daemon UID {}",
                            peer_cred.uid(),
                            current_uid
                        ),
                    ));
                }
            }

            Ok(PlatformStream::Unix(stream))
        }
    }

    #[cfg(windows)]
    fn verify_windows_peer_admission(
        server: &tokio::net::windows::named_pipe::NamedPipeServer,
    ) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use std::ptr;
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Security::{
            EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
        };
        use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };

        let raw_handle = server.as_raw_handle() as HANDLE;
        let mut client_pid: u32 = 0;
        let res = unsafe { GetNamedPipeClientProcessId(raw_handle, &mut client_pid) };
        if res == 0 {
            return Err(io::Error::last_os_error());
        }

        let my_pid = unsafe { GetCurrentProcessId() };
        if client_pid == my_pid {
            return Ok(());
        }

        unsafe {
            let proc_handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, client_pid);
            if proc_handle.is_null() || proc_handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("Failed to inspect client process PID {client_pid}"),
                ));
            }

            let mut client_token: HANDLE = ptr::null_mut();
            let mut server_token: HANDLE = ptr::null_mut();

            let ok_c = OpenProcessToken(proc_handle, TOKEN_QUERY, &mut client_token);
            CloseHandle(proc_handle);

            if ok_c == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("Failed to open client token for PID {client_pid}"),
                ));
            }

            let ok_s = OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut server_token);
            if ok_s == 0 {
                CloseHandle(client_token);
                return Err(io::Error::last_os_error());
            }

            let mut c_len: u32 = 0;
            let mut s_len: u32 = 0;
            let _ = GetTokenInformation(client_token, TokenUser, ptr::null_mut(), 0, &mut c_len);
            let _ = GetTokenInformation(server_token, TokenUser, ptr::null_mut(), 0, &mut s_len);

            let mut c_buf = vec![0u8; c_len as usize];
            let mut s_buf = vec![0u8; s_len as usize];

            let ok_c_info = GetTokenInformation(
                client_token,
                TokenUser,
                c_buf.as_mut_ptr() as *mut _,
                c_len,
                &mut c_len,
            );
            let ok_s_info = GetTokenInformation(
                server_token,
                TokenUser,
                s_buf.as_mut_ptr() as *mut _,
                s_len,
                &mut s_len,
            );

            CloseHandle(client_token);
            CloseHandle(server_token);

            if ok_c_info == 0 || ok_s_info == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Failed to retrieve user SID information from tokens",
                ));
            }

            let c_user = &*(c_buf.as_ptr() as *const TOKEN_USER);
            let s_user = &*(s_buf.as_ptr() as *const TOKEN_USER);

            if EqualSid(c_user.User.Sid, s_user.User.Sid) == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Windows peer admission denied: client user SID does not match daemon user SID",
                ));
            }
        }

        Ok(())
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
