//! OpenAI chat completions, as served by OpenAI, Ollama, LM Studio and llama.cpp.
use std::sync::Arc;

use cd_core::adapters::{AdapterError, AdapterResult};
use serde_json::{json, Value};

use super::{parse_reply, valid_name, LlmClient, Role, StructuredRequest};
use crate::http::{Request, Transport};

pub(super) struct OpenAiCompatible {
    pub endpoint: String,
    pub model: String,
    pub key: Option<String>,
    pub transport: Arc<dyn Transport>,
}

impl LlmClient for OpenAiCompatible {
    fn structured(&self, request: &StructuredRequest) -> AdapterResult<Value> {
        valid_name(request.name)?;
        let mut messages = vec![json!({"role": "system", "content": request.system})];
        messages.extend(request.messages.iter().map(|m| {
            json!({
                "role": if m.role == Role::User { "user" } else { "assistant" },
                "content": m.content,
            })
        }));
        let mut http = Request::get(format!("{}/chat/completions", self.endpoint));
        http.method = "POST";
        if let Some(key) = &self.key {
            http.headers
                .push(("Authorization".into(), format!("Bearer {key}")));
        }
        http.body = Some(json!({
            "model": self.model,
            "temperature": 0,
            "max_tokens": request.max_tokens,
            "messages": messages,
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": request.name,
                    "strict": true,
                    "schema": request.schema.provider_view(),
                },
            },
        }));
        let reply = self.transport.send(http)?.json()?;
        let choice = &reply["choices"][0];
        if choice["finish_reason"] == "length" {
            return Err(AdapterError::Invalid(
                "The model's reply was cut off by the token limit.".into(),
            ));
        }
        if choice["message"]["refusal"].is_string() {
            return Err(AdapterError::Invalid("The model declined the request.".into()));
        }
        let text = choice["message"]["content"]
            .as_str()
            .ok_or_else(|| AdapterError::Invalid("The model returned no reply.".into()))?;
        parse_reply(text)
    }
}
