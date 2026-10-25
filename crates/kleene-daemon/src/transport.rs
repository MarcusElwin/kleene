//! The byte stream between a client and the daemon.
//!
//! The daemon is addressed by a path, `.kleene/daemon.sock` by default. On
//! Unix that path is a Unix domain socket. On Windows, where tokio has no
//! Unix sockets, the path names a local named pipe instead: the pipe is
//! `\\.\pipe\kleene-<hash>`, with the hash taken from the absolute path, so
//! two workspaces get two daemons as they do on Unix, and `--socket` keeps
//! its meaning on both. Everything above this module sees a [`Listener`],
//! a [`Stream`] and [`connect`], and never the platform.

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// One connection, either end.
pub struct Stream(Box<dyn Io>);

trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stream")
    }
}

impl Stream {
    /// Split into a reading half and a writing half that can live on
    /// different tasks.
    pub fn into_split(self) -> (ReadHalf, WriteHalf) {
        tokio::io::split(self)
    }
}

/// The reading half of a [`Stream`].
pub type ReadHalf = tokio::io::ReadHalf<Stream>;
/// The writing half of a [`Stream`].
pub type WriteHalf = tokio::io::WriteHalf<Stream>;

impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_shutdown(cx)
    }
}

/// Connect to the daemon at `socket`.
pub async fn connect(socket: &Path) -> io::Result<Stream> {
    imp::connect(socket).await
}

/// Whether a daemon answers at `socket` right now. A connection is opened
/// and dropped; the daemon sees a client that leaves at once.
pub async fn is_listening(socket: &Path) -> bool {
    connect(socket).await.is_ok()
}

/// The daemon's end: bound once, accepting any number of clients.
pub struct Listener(imp::Listener);

impl Listener {
    /// Bind `socket`. Fails with [`io::ErrorKind::AddrInUse`] when another
    /// daemon is listening there; a leftover from a dead daemon is replaced.
    pub async fn bind(socket: &Path) -> io::Result<Self> {
        imp::Listener::bind(socket).await.map(Self)
    }

    /// The next client.
    pub async fn accept(&mut self) -> io::Result<Stream> {
        self.0.accept().await
    }
}

/// How the daemon's address renders: the path on Unix, the pipe name on
/// Windows.
pub fn display(socket: &Path) -> String {
    imp::display(socket)
}

#[cfg(unix)]
mod imp {
    use super::Stream;
    use std::io;
    use std::path::Path;
    use tokio::net::{UnixListener, UnixStream};

    pub async fn connect(socket: &Path) -> io::Result<Stream> {
        Ok(Stream(Box::new(UnixStream::connect(socket).await?)))
    }

    pub struct Listener(UnixListener);

    impl Listener {
        pub async fn bind(socket: &Path) -> io::Result<Self> {
            if socket.exists() {
                // A live daemon would answer; a dead one leaves the file
                // behind.
                if UnixStream::connect(socket).await.is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        format!("a daemon is already listening on {}", socket.display()),
                    ));
                }
                std::fs::remove_file(socket)?;
            }
            if let Some(parent) = socket.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Ok(Self(UnixListener::bind(socket)?))
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            let (stream, _) = self.0.accept().await?;
            Ok(Stream(Box::new(stream)))
        }
    }

    pub fn display(socket: &Path) -> String {
        socket.display().to_string()
    }
}

#[cfg(windows)]
mod imp {
    use super::Stream;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::io;
    use std::path::Path;
    use std::time::Duration;
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

    /// `ERROR_PIPE_BUSY`: every instance is taken; another is on its way.
    const PIPE_BUSY: i32 = 231;
    /// `ERROR_ACCESS_DENIED`: what `first_pipe_instance` reports when the
    /// pipe already exists.
    const ACCESS_DENIED: i32 = 5;

    /// The pipe for a socket path: stable for the same workspace, distinct
    /// for different ones.
    pub fn pipe_name(socket: &Path) -> String {
        let absolute = std::path::absolute(socket).unwrap_or_else(|_| socket.to_path_buf());
        let mut h = DefaultHasher::new();
        absolute.to_string_lossy().to_lowercase().hash(&mut h);
        format!(r"\\.\pipe\kleene-{:016x}", h.finish())
    }

    pub async fn connect(socket: &Path) -> io::Result<Stream> {
        let name = pipe_name(socket);
        // The server re-creates an instance right after each accept; a
        // client that lands in that gap sees PIPE_BUSY and tries again.
        for _ in 0..20 {
            match ClientOptions::new().open(&name) {
                Ok(c) => return Ok(Stream(Box::new(c))),
                Err(e) if e.raw_os_error() == Some(PIPE_BUSY) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("{name} stayed busy"),
        ))
    }

    pub struct Listener {
        name: String,
        next: NamedPipeServer,
    }

    impl Listener {
        pub async fn bind(socket: &Path) -> io::Result<Self> {
            let name = pipe_name(socket);
            let next = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)
                .map_err(|e| {
                    if e.raw_os_error() == Some(ACCESS_DENIED) {
                        io::Error::new(
                            io::ErrorKind::AddrInUse,
                            format!("a daemon is already listening on {name}"),
                        )
                    } else {
                        e
                    }
                })?;
            Ok(Self { name, next })
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            self.next.connect().await?;
            let connected =
                std::mem::replace(&mut self.next, ServerOptions::new().create(&self.name)?);
            Ok(Stream(Box::new(connected)))
        }
    }

    pub fn display(socket: &Path) -> String {
        format!("{} ({})", socket.display(), pipe_name(socket))
    }
}

#[cfg(not(any(unix, windows)))]
compile_error!("kleene-daemon needs Unix sockets or Windows named pipes");

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn round_trip_and_stale_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("nested").join("daemon.sock");
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        std::fs::write(&socket, b"").unwrap(); // a dead daemon's leftover
        assert!(!is_listening(&socket).await);
        let mut listener = Listener::bind(&socket).await.unwrap();
        let server = tokio::spawn(async move {
            let mut s = listener.accept().await.unwrap();
            let mut buf = [0u8; 2];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(b"pong").await.unwrap();
            (buf, listener)
        });
        let mut c = connect(&socket).await.unwrap();
        c.write_all(b"hi").await.unwrap();
        let mut reply = String::new();
        let (mut r, mut w) = c.into_split();
        w.shutdown().await.unwrap();
        r.read_to_string(&mut reply).await.unwrap();
        let (got, listener) = server.await.unwrap();
        assert_eq!(&got, b"hi");
        assert_eq!(reply, "pong");
        // While the first listener lives, a second daemon is refused; the
        // probe it makes is a client the listener never accepts.
        assert!(is_listening(&socket).await);
        assert_eq!(
            Listener::bind(&socket)
                .await
                .map(|_| ())
                .unwrap_err()
                .kind(),
            io::ErrorKind::AddrInUse
        );
        drop(listener);
    }
}
