//! Server-sent events: an incremental line parser over a byte stream.
//!
//! Both HTTP adapters stream over SSE. The parser follows the WHATWG
//! event-stream grammar for the parts the model APIs use: `event:` and
//! `data:` fields, multi-line data joined with `\n`, comment lines, `\r\n`
//! line endings, and lines split across chunk boundaries. It is fed raw
//! chunks and hands back complete events; [`events`] wraps it around any
//! byte stream, so a fixture transcript and a network response go through
//! the same code.

use crate::types::{ProviderError, StreamEvent};
use bytes::Bytes;
use futures::stream::{BoxStream, Stream, StreamExt};
use std::fmt::Display;

/// One server-sent event.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    /// The `event:` field, if the server sent one.
    pub event: Option<String>,
    /// All `data:` lines of the event, joined with `\n`.
    pub data: String,
}

/// Incremental SSE parser. Feed it chunks as they arrive; it returns every
/// event completed by that chunk and keeps a partial line for the next one.
#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    /// An empty parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume a chunk and return the events it completed.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut start = 0;
        while let Some(nl) = self.buf[start..].iter().position(|b| *b == b'\n') {
            let end = start + nl;
            let line = Self::decode_line(&self.buf[start..end]);
            if let Some(ev) = self.line(&line) {
                out.push(ev);
            }
            start = end + 1;
        }
        self.buf.drain(..start);
        out
    }

    /// Flush at end of stream: a trailing line without a newline is still
    /// processed and a pending event is dispatched.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            let line = Self::decode_line(&rest);
            if let Some(ev) = self.line(&line) {
                out.push(ev);
            }
        }
        if let Some(ev) = self.dispatch() {
            out.push(ev);
        }
        out
    }

    fn decode_line(raw: &[u8]) -> String {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        String::from_utf8_lossy(raw).into_owned()
    }

    fn line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        let event = self.event.take();
        if self.data.is_empty() {
            return None;
        }
        let data = std::mem::take(&mut self.data).join("\n");
        Some(SseEvent { event, data })
    }
}

/// Parse a byte stream into events. A transport error surfaces as
/// [`ProviderError::Network`] and ends the stream.
pub fn events<S, E>(bytes: S) -> BoxStream<'static, Result<SseEvent, ProviderError>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Display + Send + 'static,
{
    let state = (bytes.boxed(), SseParser::new(), false);
    futures::stream::unfold(state, |(mut src, mut parser, done)| async move {
        if done {
            return None;
        }
        let (items, done): (Vec<Result<SseEvent, ProviderError>>, bool) = match src.next().await {
            Some(Ok(chunk)) => (parser.feed(&chunk).into_iter().map(Ok).collect(), false),
            Some(Err(e)) => (vec![Err(ProviderError::Network(e.to_string()))], true),
            None => (parser.finish().into_iter().map(Ok).collect(), true),
        };
        Some((futures::stream::iter(items), (src, parser, done)))
    })
    .flatten()
    .boxed()
}

/// Turns vendor SSE events into provider-neutral [`StreamEvent`]s and
/// assembles the final response. Each adapter implements one.
pub(crate) trait Assembler: Send + 'static {
    /// Handle one event; return the stream events it produces. Returning a
    /// [`StreamEvent::Done`] ends the stream.
    fn on_event(&mut self, ev: SseEvent) -> Result<Vec<StreamEvent>, ProviderError>;
    /// The byte stream ended without the vendor's terminator.
    fn finish(&mut self) -> Result<Vec<StreamEvent>, ProviderError>;
}

/// Drive an [`Assembler`] over a byte stream.
pub(crate) fn assemble<S, E, A>(
    bytes: S,
    assembler: A,
) -> BoxStream<'static, Result<StreamEvent, ProviderError>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Display + Send + 'static,
    A: Assembler,
{
    let state = (events(bytes), assembler, false);
    futures::stream::unfold(state, |(mut src, mut asm, done)| async move {
        if done {
            return None;
        }
        let (items, done): (Vec<Result<StreamEvent, ProviderError>>, bool) = match src.next().await
        {
            Some(Ok(ev)) => match asm.on_event(ev) {
                Ok(evs) => {
                    let done = evs.iter().any(|e| matches!(e, StreamEvent::Done(_)));
                    (evs.into_iter().map(Ok).collect(), done)
                }
                Err(e) => (vec![Err(e)], true),
            },
            Some(Err(e)) => (vec![Err(e)], true),
            None => match asm.finish() {
                Ok(evs) => (evs.into_iter().map(Ok).collect(), true),
                Err(e) => (vec![Err(e)], true),
            },
        };
        Some((futures::stream::iter(items), (src, asm, done)))
    })
    .flatten()
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    fn ev(event: Option<&str>, data: &str) -> SseEvent {
        SseEvent {
            event: event.map(str::to_string),
            data: data.to_string(),
        }
    }

    #[test]
    fn parses_event_and_data_lines() {
        let mut p = SseParser::new();
        let out = p.feed(b"event: ping\ndata: {\"a\":1}\n\n");
        assert_eq!(out, vec![ev(Some("ping"), "{\"a\":1}")]);
    }

    #[test]
    fn joins_multi_line_data_and_ignores_comments_and_unknown_fields() {
        let mut p = SseParser::new();
        let out = p.feed(b": keep-alive\nid: 7\ndata: first\ndata:second\nretry: 100\n\n");
        assert_eq!(out, vec![ev(None, "first\nsecond")]);
    }

    #[test]
    fn handles_crlf() {
        let mut p = SseParser::new();
        let out = p.feed(b"event: x\r\ndata: y\r\n\r\n");
        assert_eq!(out, vec![ev(Some("x"), "y")]);
    }

    #[test]
    fn reassembles_lines_split_across_chunks() {
        let mut p = SseParser::new();
        assert!(p.feed(b"eve").is_empty());
        assert!(p.feed(b"nt: message_start\nda").is_empty());
        assert!(p.feed(b"ta: {\"ty").is_empty());
        let out = p.feed(b"pe\":\"m\"}\n\nevent: b\ndata: 2\n");
        assert_eq!(out, vec![ev(Some("message_start"), "{\"type\":\"m\"}")]);
        // The second event completes only once its blank line arrives.
        assert_eq!(p.feed(b"\n"), vec![ev(Some("b"), "2")]);
    }

    #[test]
    fn split_inside_a_multibyte_character_is_safe() {
        let mut p = SseParser::new();
        let text = "data: héllo\n\n".as_bytes();
        // 'é' is two bytes; split between them.
        let cut = text.iter().position(|b| *b == 0xC3).unwrap() + 1;
        assert!(p.feed(&text[..cut]).is_empty());
        assert_eq!(p.feed(&text[cut..]), vec![ev(None, "héllo")]);
    }

    #[test]
    fn finish_flushes_a_trailing_event_without_blank_line() {
        let mut p = SseParser::new();
        assert!(p.feed(b"data: [DONE]").is_empty());
        assert_eq!(p.finish(), vec![ev(None, "[DONE]")]);
        assert!(p.finish().is_empty());
    }

    #[test]
    fn event_without_data_is_dropped() {
        let mut p = SseParser::new();
        assert!(p.feed(b"event: nothing\n\n").is_empty());
    }

    #[tokio::test]
    async fn events_stream_over_chunks() {
        let chunks = vec![
            Ok::<_, Infallible>(Bytes::from_static(b"event: a\nda")),
            Ok(Bytes::from_static(b"ta: 1\n\nevent: b\ndata: 2\n\n")),
        ];
        let got: Vec<_> = events(futures::stream::iter(chunks))
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert_eq!(got, vec![ev(Some("a"), "1"), ev(Some("b"), "2")]);
    }

    #[tokio::test]
    async fn transport_error_ends_the_stream_with_network() {
        #[derive(Debug)]
        struct Boom;
        impl Display for Boom {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "boom")
            }
        }
        let chunks = vec![
            Ok(Bytes::from_static(b"data: 1\n\n")),
            Err(Boom),
            Ok(Bytes::from_static(b"data: 2\n\n")),
        ];
        let got: Vec<_> = events(futures::stream::iter(chunks)).collect().await;
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].as_ref().unwrap().data, "1");
        assert!(matches!(got[1], Err(ProviderError::Network(ref m)) if m == "boom"));
    }
}
