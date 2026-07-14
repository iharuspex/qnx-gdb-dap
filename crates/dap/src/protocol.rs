use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A request sent from a DAP client to the debug adapter.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Request {
    /// Sequence number assigned by the client.
    pub seq: u64,

    /// Must contain `"request"`.
    #[serde(rename = "type")]
    pub message_type: RequestMessageType,

    /// Name of the requested operation.
    pub command: String,

    /// Command-specific arguments.
    #[serde(default)]
    pub arguments: Option<Value>,
}

/// Type discriminator for incoming requests.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RequestMessageType {
    Request,
}

/// A response sent by the debug adapter.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Response {
    /// Sequence number assigned by the adapter.
    pub seq: u64,

    /// Must contain `"response"`.
    #[serde(rename = "type")]
    pub message_type: ResponseMessageType,

    /// Sequence number of the corresponding request.
    pub request_seq: u64,

    /// Whether the request completed successfully.
    pub success: bool,

    /// Name of the original request.
    pub command: String,

    /// Human-readable error description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Command-specific response data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

impl Response {
    /// Creates a successful response.
    #[must_use]
    pub fn success(seq: u64, request: &Request, body: Option<Value>) -> Self {
        Self {
            seq,
            message_type: ResponseMessageType::Response,
            request_seq: request.seq,
            success: true,
            command: request.command.clone(),
            message: None,
            body,
        }
    }

    /// Creates an unsuccessful response.
    pub fn error(seq: u64, request: &Request, message: impl Into<String>) -> Self {
        Self {
            seq,
            message_type: ResponseMessageType::Response,
            request_seq: request.seq,
            success: false,
            command: request.command.clone(),
            message: Some(message.into()),
            body: None,
        }
    }
}

/// Type discriminator for responses.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ResponseMessageType {
    Response,
}

/// An asynchronous event sent by the debug adapter.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Event {
    /// Sequence number assigned by the adapter.
    pub seq: u64,

    /// Must contain `"event"`.
    #[serde(rename = "type")]
    pub message_type: EventMessageType,

    /// Name of the event.
    pub event: String,

    /// Event-specific data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

impl Event {
    /// Creates an event without a body.
    pub fn new(seq: u64, event: impl Into<String>) -> Self {
        Self {
            seq,
            message_type: EventMessageType::Event,
            event: event.into(),
            body: None,
        }
    }

    /// Creates an event with a JSON body.
    pub fn with_body(seq: u64, event: impl Into<String>, body: Value) -> Self {
        Self {
            seq,
            message_type: EventMessageType::Event,
            event: event.into(),
            body: Some(body),
        }
    }
}

/// Type discriminator for events.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EventMessageType {
    Event,
}

/// Any message that can be sent by the adapter.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(untagged)]
pub enum OutgoingMessage {
    Response(Response),
    Event(Event),
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Event, OutgoingMessage, Request, Response};

    #[test]
    fn deserializes_request() {
        let value = json!({
            "seq": 1,
            "type": "request",
            "command": "initialize",
            "arguments": {
                "clientID": "zed"
            }
        });

        let request: Request = serde_json::from_value(value).expect("request should deserialize");

        assert_eq!(request.seq, 1);
        assert_eq!(request.command, "initialize");
        assert_eq!(
            request.arguments,
            Some(json!({
                "clientID": "zed"
            }))
        );
    }

    #[test]
    fn serializes_success_response() {
        let request: Request = serde_json::from_value(json!({
            "seq": 12,
            "type": "request",
            "command": "initialize"
        }))
        .expect("request should deserialize");

        let response = Response::success(
            1,
            &request,
            Some(json!({
                "supportsConfigurationDoneRequest": false
            })),
        );

        let value = serde_json::to_value(response).expect("response should serialize");

        assert_eq!(
            value,
            json!({
                "seq": 1,
                "type": "response",
                "request_seq": 12,
                "success": true,
                "command": "initialize",
                "body": {
                    "supportsConfigurationDoneRequest": false
                }
            })
        );
    }

    #[test]
    fn serializes_error_response() {
        let request: Request = serde_json::from_value(json!({
            "seq": 15,
            "type": "request",
            "command": "launch"
        }))
        .expect("request should deserialize");

        let response = Response::error(2, &request, "command is not implemented");

        let value = serde_json::to_value(response).expect("response should serialize");

        assert_eq!(
            value,
            json!({
                "seq": 2,
                "type": "response",
                "request_seq": 15,
                "success": false,
                "command": "launch",
                "message": "command is not implemented"
            })
        );
    }

    #[test]
    fn serializes_initialized_event() {
        let event = Event::new(3, "initialized");
        let message = OutgoingMessage::Event(event);

        let value: Value = serde_json::to_value(message).expect("event should serialize");

        assert_eq!(
            value,
            json!({
                "seq": 3,
                "type": "event",
                "event": "initialized"
            })
        );
    }
}
