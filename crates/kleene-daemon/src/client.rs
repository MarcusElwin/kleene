//! A client of the daemon socket: JSON lines in, JSON lines out.

use crate::{ClientRequest, DaemonError, ServerMessage};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

/// The sending half of a connection.
pub struct ClientWriter {
    write: OwnedWriteHalf,
}

impl ClientWriter {
    /// Send one request.
    pub async fn send(&mut self, req: &ClientRequest) -> Result<(), DaemonError> {
        let mut line =
            serde_json::to_string(req).map_err(|e| DaemonError::Protocol(e.to_string()))?;
        line.push('\n');
        self.write.write_all(line.as_bytes()).await?;
        Ok(())
    }
}

/// The receiving half of a connection.
pub struct ClientReader {
    lines: Lines<BufReader<OwnedReadHalf>>,
}

impl ClientReader {
    /// The next message, or `None` when the daemon closed the connection.
    pub async fn recv(&mut self) -> Result<Option<ServerMessage>, DaemonError> {
        loop {
            match self.lines.next_line().await? {
                None => return Ok(None),
                Some(l) if l.trim().is_empty() => continue,
                Some(l) => {
                    return serde_json::from_str(&l)
                        .map(Some)
                        .map_err(|e| DaemonError::Protocol(format!("{e}: {l}")))
                }
            }
        }
    }
}

/// A connection to the daemon.
pub struct Client {
    reader: ClientReader,
    writer: ClientWriter,
    /// The daemon's generation from its `Hello`.
    pub generation: u64,
    /// The daemon's kleene version from its `Hello`, `None` from a daemon
    /// too old to send one.
    pub version: Option<String>,
}

impl Client {
    /// Connect and complete the handshake.
    pub async fn connect(socket: &Path) -> Result<Self, DaemonError> {
        let stream = UnixStream::connect(socket).await?;
        let (read, write) = stream.into_split();
        let mut reader = ClientReader {
            lines: BufReader::new(read).lines(),
        };
        let writer = ClientWriter { write };
        let (generation, version) = match reader.recv().await? {
            Some(ServerMessage::Hello {
                protocol,
                generation,
                version,
            }) => {
                if protocol != crate::PROTOCOL_VERSION {
                    return Err(DaemonError::Protocol(format!(
                        "daemon speaks protocol {protocol}, this client {}",
                        crate::PROTOCOL_VERSION
                    )));
                }
                (generation, version)
            }
            Some(other) => {
                return Err(DaemonError::Protocol(format!(
                    "expected hello, got {other:?}"
                )))
            }
            None => return Err(DaemonError::Closed),
        };
        Ok(Self {
            reader,
            writer,
            generation,
            version,
        })
    }

    /// Send one request.
    pub async fn send(&mut self, req: &ClientRequest) -> Result<(), DaemonError> {
        self.writer.send(req).await
    }

    /// The next message.
    pub async fn recv(&mut self) -> Result<Option<ServerMessage>, DaemonError> {
        self.reader.recv().await
    }

    /// Send a request and wait for its reply, skipping streamed events. Only
    /// sensible before `Subscribe`, or on a connection that never subscribes.
    pub async fn call(&mut self, req: &ClientRequest) -> Result<ServerMessage, DaemonError> {
        self.send(req).await?;
        loop {
            match self.recv().await? {
                None => return Err(DaemonError::Closed),
                Some(
                    ServerMessage::Event { .. }
                    | ServerMessage::CallDelta { .. }
                    | ServerMessage::TurnFinished { .. },
                ) => continue,
                Some(ServerMessage::RunFinished { .. })
                    if !matches!(req, ClientRequest::CancelRun { .. }) =>
                {
                    continue
                }
                Some(m) => return Ok(m),
            }
        }
    }

    /// Split into independently usable halves (a reader task and a writer).
    pub fn into_split(self) -> (ClientReader, ClientWriter) {
        (self.reader, self.writer)
    }
}
