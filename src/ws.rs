// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! A WebSocket to a relay, text frames only: what the listener and the
//! probes need.
//!
//! Native build: tokio-tungstenite with rustls, as before. Browser build: the
//! browser's `WebSocket`, which hides why a connection failed (no TLS, DNS
//! or HTTP detail): every failure there may recover, except a malformed
//! address.

use std::fmt;

/// Why a connection failed or ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsError {
    text: String,
    permanent: bool,
}

impl WsError {
    /// A retry won't fix it: bad certificate, unknown host, connection
    /// refused, HTTP 4xx, malformed address.
    pub fn is_permanent(&self) -> bool {
        self.permanent
    }
}

impl fmt::Display for WsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for WsError {}

/// What comes from the relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Close,
    /// Binary, ping, pong: nothing for us.
    Other,
}

#[cfg(target_arch = "wasm32")]
pub use browser::Socket;
#[cfg(not(target_arch = "wasm32"))]
pub use native::Socket;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::ErrorKind;

    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Error, Message};

    use super::{Frame, WsError};

    type Stream = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    pub struct Socket {
        inner: Stream,
    }

    impl From<Error> for WsError {
        fn from(e: Error) -> Self {
            WsError {
                permanent: is_permanent(&e),
                text: e.to_string(),
            }
        }
    }

    /// rustls certificate and handshake errors reach us as
    /// `Io(InvalidData)`; DNS failures as an uncategorised I/O error.
    fn is_permanent(e: &Error) -> bool {
        match e {
            Error::Tls(_) | Error::Url(_) => true,
            Error::Http(response) => response.status().is_client_error(),
            Error::Io(io) => {
                matches!(
                    io.kind(),
                    ErrorKind::ConnectionRefused | ErrorKind::InvalidData
                ) || io.to_string().contains("failed to lookup address")
            }
            _ => false,
        }
    }

    impl Socket {
        pub async fn connect(url: &str) -> Result<Socket, WsError> {
            let (inner, _) = tokio_tungstenite::connect_async(url).await?;
            Ok(Socket { inner })
        }

        pub async fn send_text(&mut self, text: String) -> Result<(), WsError> {
            Ok(self.inner.send(Message::text(text)).await?)
        }

        /// The next frame; `None` once the connection is gone. Cancel-safe.
        pub async fn next(&mut self) -> Option<Result<Frame, WsError>> {
            Some(match self.inner.next().await? {
                Ok(Message::Text(t)) => Ok(Frame::Text(t.as_str().to_owned())),
                Ok(Message::Close(_)) => Ok(Frame::Close),
                Ok(_) => Ok(Frame::Other),
                Err(e) => Err(e.into()),
            })
        }

        pub async fn close(&mut self) {
            let _ = self.inner.close(None).await;
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use tokio::sync::mpsc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;
    use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

    use super::{Frame, WsError};

    enum Incoming {
        Open,
        Frame(Frame),
        Error,
    }

    pub struct Socket {
        ws: WebSocket,
        rx: mpsc::UnboundedReceiver<Incoming>,
        // Kept alive as long as the socket: the browser calls them.
        _on_open: Closure<dyn FnMut(Event)>,
        _on_message: Closure<dyn FnMut(MessageEvent)>,
        _on_error: Closure<dyn FnMut(Event)>,
        _on_close: Closure<dyn FnMut(CloseEvent)>,
    }

    fn failed(text: impl Into<String>) -> WsError {
        WsError {
            text: text.into(),
            permanent: false,
        }
    }

    impl Socket {
        pub async fn connect(url: &str) -> Result<Socket, WsError> {
            // Throws only for a malformed address: that won't recover.
            let ws = WebSocket::new(url).map_err(|e| WsError {
                text: format!("invalid address: {}", e.as_string().unwrap_or_default()),
                permanent: true,
            })?;
            ws.set_binary_type(BinaryType::Arraybuffer);
            let (tx, rx) = mpsc::unbounded_channel();
            let sender = tx.clone();
            let on_open = Closure::<dyn FnMut(Event)>::new(move |_| {
                let _ = sender.send(Incoming::Open);
            });
            let sender = tx.clone();
            let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                let frame = match e.data().as_string() {
                    Some(text) => Frame::Text(text),
                    None => Frame::Other,
                };
                let _ = sender.send(Incoming::Frame(frame));
            });
            let sender = tx.clone();
            let on_error = Closure::<dyn FnMut(Event)>::new(move |_| {
                let _ = sender.send(Incoming::Error);
            });
            let on_close = Closure::<dyn FnMut(CloseEvent)>::new(move |_| {
                let _ = tx.send(Incoming::Frame(Frame::Close));
            });
            ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
            ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
            ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
            let mut socket = Socket {
                ws,
                rx,
                _on_open: on_open,
                _on_message: on_message,
                _on_error: on_error,
                _on_close: on_close,
            };
            // Dropped on failure, which closes it.
            match socket.rx.recv().await {
                Some(Incoming::Open) => Ok(socket),
                _ => Err(failed("connection failed")),
            }
        }

        pub async fn send_text(&mut self, text: String) -> Result<(), WsError> {
            self.ws
                .send_with_str(&text)
                .map_err(|_| failed("send failed: connection not open"))
        }

        /// The next frame; `None` once the connection is gone. Cancel-safe.
        pub async fn next(&mut self) -> Option<Result<Frame, WsError>> {
            match self.rx.recv().await? {
                Incoming::Frame(frame) => Some(Ok(frame)),
                Incoming::Error => Some(Err(failed("connection error"))),
                Incoming::Open => Some(Ok(Frame::Other)),
            }
        }

        pub async fn close(&mut self) {
            let _ = self.ws.close();
        }
    }

    impl Drop for Socket {
        fn drop(&mut self) {
            self.ws.set_onopen(None);
            self.ws.set_onmessage(None);
            self.ws.set_onerror(None);
            self.ws.set_onclose(None);
            let _ = self.ws.close();
        }
    }
}
