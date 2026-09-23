//! Anthropic Messages API. Structured replies use one forced tool whose input
//! schema is the reply schema.
use std::sync::Arc;

use cd_core::adapters::{AdapterError, AdapterResult};
use serde_json::{json, Value};

use super::{valid_name, LlmClient, Role, StructuredRequest};
use crate::http::{Request, Transport};

pub(super) struct Anthropic {
    pub endpoint: String,
    pub model: String,
    pub key: String,
    pub transport: Arc<dyn Transport>,
}

impl LlmClient for Anthropic {
    fn structured(&self, request: &StructuredRequest) -> AdapterResult<Value> {
        valid_name(request.name)?;
        // The API expects alternating roles; merge consecutive messages from one side.
        let mut messages: Vec<(Role, String)> = vec![];
        for m in request.messages {
            match messages.last_mut() {
                Some((role, text)) if *role == m.role => {
                    text.push_str("\n\n");
                    text.push_str(&m.content);
                }
                _ => messages.push((m.role, m.content.clone())),
            }
        }
        let messages: Vec<Value> = messages
            .into_iter()
            .map(|(role, content)| {
                json!({
                    "role": if role == Role::User { "user" } else { "assistant" },
                    "content": content,
                })
            })
            .collect();
        let mut http = Request::get(format!("{}/messages", self.endpoint));
        http.method = "POST";
        http.headers.push(("x-api-key".into(), self.key.clone()));
        http.headers
            .push(("anthropic-version".into(), "2023-06-01".into()));
        http.body = Some(json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "temperature": 0,
            "system": request.system,
            "messages": messages,
            "tools": [{
                "name": request.name,
                "description": "Give your reply in this format.",
                "input_schema": request.schema.provider_view(),
            }],
            "tool_choice": {"type": "tool", "name": request.name},
        }));
        let reply = self.transport.send(http)?.json()?;
        if reply["stop_reason"] == "max_tokens" {
            return Err(AdapterError::Invalid(
                "The model's reply was cut off by the token limit.".into(),
            ));
        }
        reply["content"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|block| block["type"] == "tool_use" && block["name"] == request.name)
            .map(|block| block["input"].clone())
            .ok_or_else(|| AdapterError::Invalid("The model returned no structured reply.".into()))
    }
}
