// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{send_error, RequestId};
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{self, BufRead},
    sync::Arc,
};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

type RequestHandler<C> = Arc<dyn Fn(Arc<C>, RequestId, Value)>;
type NotificationHandler<C> = Arc<dyn Fn(Arc<C>, Value)>;

pub struct HandlersKeyedByMethodName<C> {
    context: Arc<C>,
    requests: HashMap<&'static str, RequestHandler<C>>,
    notifications: HashMap<&'static str, NotificationHandler<C>>,
}

#[derive(Debug)]
struct RequestError {
    id: Option<RequestId>,
    code: i32,
    message: String,
}

impl<C> HandlersKeyedByMethodName<C> {
    pub fn new(context: Arc<C>) -> Self {
        Self {
            context,
            requests: HashMap::new(),
            notifications: HashMap::new(),
        }
    }

    pub fn add_request_handler<F>(&mut self, method: &'static str, handler: F)
    where
        F: Fn(Arc<C>, RequestId, Value) + Send + Sync + 'static,
    {
        self.requests.insert(method, Arc::new(handler));
    }

    pub fn add_notification_handler<F>(&mut self, method: &'static str, handler: F)
    where
        F: Fn(Arc<C>, Value) + Send + Sync + 'static,
    {
        self.notifications.insert(method, Arc::new(handler));
    }

    fn handle_request(&self, message: Value) -> Option<RequestError> {
        let id = message.get("id").cloned();
        let valid_id = id
            .as_ref()
            .is_none_or(|id| id.is_null() || id.is_string() || id.is_number());
        let method = message.get("method").and_then(Value::as_str);
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || method.is_none()
            || !valid_id
        {
            return Some(RequestError {
                id: None,
                code: -32600,
                message: "Invalid JSON-RPC request".to_string(),
            });
        }
        let method = method.expect("method validated above");
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        if !params.is_null() && !params.is_object() && !params.is_array() {
            // Notifications never receive responses, including invalid params.
            return id.map(|id| RequestError {
                id: Some(id),
                code: -32602,
                message: "Invalid params".to_string(),
            });
        }
        if let Some(id) = id {
            if let Some(handler) = self.requests.get(method) {
                handler(self.context.clone(), id, params);
                None
            } else {
                Some(RequestError {
                    id: Some(id),
                    code: -32601,
                    message: format!("Method not found: {method}"),
                })
            }
        } else {
            if let Some(handler) = self.notifications.get(method) {
                handler(self.context.clone(), params);
            }
            None
        }
    }
}

/// Serve complete frames until the client closes stdin or framing is invalid.
pub fn start_server<C>(handlers: &HandlersKeyedByMethodName<C>) {
    if let Err(error) = serve(&mut io::stdin().lock(), handlers) {
        eprintln!("JSON-RPC input closed: {error}");
    }
}

fn serve<R: BufRead, C>(reader: &mut R, handlers: &HandlersKeyedByMethodName<C>) -> io::Result<()> {
    while let Some(body) = read_message(reader)? {
        match serde_json::from_slice::<Value>(&body) {
            Ok(message) => {
                if let Some(error) = handlers.handle_request(message) {
                    send_error(error.id, error.code, error.message);
                }
            }
            Err(_) => send_error(None, -32700, "Parse error".to_string()),
        }
    }
    Ok(())
}

fn invalid_frame(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut header_bytes = 0;
    let mut content_length = None;
    loop {
        let mut line = Vec::new();
        let read = io::Read::take(&mut *reader, (MAX_HEADER_BYTES - header_bytes + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return if header_bytes == 0 {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Incomplete header",
                ))
            };
        }
        header_bytes += read;
        if header_bytes > MAX_HEADER_BYTES {
            return Err(invalid_frame("Header exceeds size limit"));
        }
        let line = std::str::from_utf8(&line)
            .map_err(|_| invalid_frame("Header is not UTF-8"))?
            .trim();
        if line.is_empty() {
            break;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid_frame("Malformed header"))?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if content_length.is_some() {
                return Err(invalid_frame("Duplicate Content-Length"));
            }
            let length = value
                .trim()
                .parse::<usize>()
                .map_err(|_| invalid_frame("Invalid Content-Length"))?;
            if length > MAX_MESSAGE_BYTES {
                return Err(invalid_frame("Message exceeds size limit"));
            }
            content_length = Some(length);
        }
    }
    let length = content_length.ok_or_else(|| invalid_frame("Missing Content-Length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;
    use std::sync::Mutex;

    #[test]
    fn eof_finishes_without_dispatching() {
        let handlers = HandlersKeyedByMethodName::new(Arc::new(()));
        serve(&mut Cursor::new(Vec::<u8>::new()), &handlers).unwrap();
    }

    #[test]
    fn reads_additional_headers_and_consecutive_frames() {
        let first = br#"{"jsonrpc":"2.0","id":1,"method":"configure"}"#;
        let second = br#"{"jsonrpc":"2.0","id":"two","method":"configure"}"#;
        let mut wire = format!(
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length:{}\r\n\r\n",
            first.len()
        )
        .into_bytes();
        wire.extend_from_slice(first);
        wire.extend_from_slice(format!("Content-Length: {}\r\n\r\n", second.len()).as_bytes());
        wire.extend_from_slice(second);
        let mut reader = Cursor::new(wire);
        assert_eq!(read_message(&mut reader).unwrap(), Some(first.to_vec()));
        assert_eq!(read_message(&mut reader).unwrap(), Some(second.to_vec()));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn rejects_incomplete_invalid_and_oversized_frames() {
        for wire in [
            "Content-Length: 2\r\n",
            "Content-Length: 2\r\n\r\n{",
            "Content-Length: 1\r\nContent-Length: 1\r\n\r\nx",
            "Content-Length: invalid\r\n\r\n",
            "Content-Type: application/json\r\n\r\n",
        ] {
            assert!(read_message(&mut Cursor::new(wire)).is_err(), "{wire:?}");
        }
        let oversized = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1);
        assert!(read_message(&mut Cursor::new(oversized)).is_err());
        assert!(read_message(&mut Cursor::new(vec![b'x'; MAX_HEADER_BYTES + 1])).is_err());
    }

    #[test]
    fn preserves_string_large_signed_and_null_request_ids() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let mut handlers = HandlersKeyedByMethodName::new(received.clone());
        handlers.add_request_handler("configure", |received, id, _| {
            received.lock().unwrap().push(id);
        });
        let ids = vec![
            json!("request-a"),
            json!(4294967297u64),
            json!(-7),
            Value::Null,
        ];
        for id in &ids {
            assert!(handlers
                .handle_request(json!({
                    "jsonrpc": "2.0", "id": id, "method": "configure"
                }))
                .is_none());
        }
        assert_eq!(*received.lock().unwrap(), ids);
    }

    #[test]
    fn unknown_notifications_are_silent_and_unknown_requests_keep_their_id() {
        let handlers = HandlersKeyedByMethodName::new(Arc::new(()));
        assert!(handlers
            .handle_request(json!({"jsonrpc": "2.0", "method": "unknown"}))
            .is_none());
        let error = handlers
            .handle_request(json!({
                "jsonrpc": "2.0", "id": "missing", "method": "unknown"
            }))
            .unwrap();
        assert_eq!(error.code, -32601);
        assert_eq!(error.id, Some(json!("missing")));
    }

    #[test]
    fn rejects_invalid_envelopes_and_ids() {
        let handlers = HandlersKeyedByMethodName::new(Arc::new(()));
        for message in [
            json!({"jsonrpc": "1.0", "id": 1, "method": "configure"}),
            json!({"jsonrpc": "2.0", "id": {}, "method": "configure"}),
            json!({"jsonrpc": "2.0", "id": 1}),
        ] {
            assert_eq!(handlers.handle_request(message).unwrap().code, -32600);
        }
    }
}
