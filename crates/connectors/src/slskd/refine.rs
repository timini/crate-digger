//! When plain searches find no acceptable copy, a model may try other
//! search words. It sees only file names and counts; every result it turns
//! up still goes through the matching rule, and it cannot start downloads.
use std::sync::Mutex;

use cd_core::adapters::{AcquisitionQuery, AdapterError, AdapterResult, SearchResult};
use serde_json::{json, Value};

use super::{remote_name, Slskd};
use crate::llm::agent::{run, Limits, Tool, Toolbox};
use crate::llm::schema::Schema;
use crate::llm::{untrusted, LlmClient, Message};

const MAX_SEARCHES: usize = 3;

struct SearchTool<'a> {
    slskd: &'a Slskd,
    tools: Vec<Tool>,
    found: Mutex<Vec<SearchResult>>,
    searches: Mutex<usize>,
}

impl Toolbox for SearchTool<'_> {
    fn tools(&self) -> &[Tool] {
        &self.tools
    }

    fn call(&self, _: &str, arguments: &Value) -> AdapterResult<Value> {
        {
            let mut n = self.searches.lock().unwrap();
            if *n >= MAX_SEARCHES {
                return Err(AdapterError::Invalid("No searches left; finish now.".into()));
            }
            *n += 1;
        }
        let text = arguments["text"].as_str().unwrap_or_default();
        let results = self.slskd.search_text(text)?;
        let sample: Vec<&str> = results
            .iter()
            .take(15)
            .map(|r| remote_name(&r.filename))
            .collect();
        let summary = json!({"results": results.len(), "file_names": sample});
        self.found.lock().unwrap().extend(results);
        Ok(summary)
    }
}

pub fn refine(
    model: &dyn LlmClient,
    slskd: &Slskd,
    query: &AcquisitionQuery,
) -> AdapterResult<Vec<SearchResult>> {
    let toolbox = SearchTool {
        slskd,
        tools: vec![Tool {
            name: "soulseek_search",
            description: "Search Soulseek. Every word must appear in a file's path. Returns how many files matched and some file names.",
            arguments: Schema::new(json!({
                "type": "object",
                "properties": {"text": {"type": "string", "minLength": 2, "maxLength": 100}},
                "required": ["text"],
                "additionalProperties": false
            }))
            .expect("static schema"),
        }],
        found: Mutex::default(),
        searches: Mutex::new(0),
    };
    let wanted = format!(
        "artist: {}\ntitle: {}\nmix: {}",
        query.artist,
        query.title,
        query.mix.as_deref().unwrap_or("not named")
    );
    let result = Schema::new(json!({
        "type": "object",
        "properties": {"note": {"type": "string", "maxLength": 200}},
        "required": ["note"],
        "additionalProperties": false
    }))
    .expect("static schema");
    run(
        model,
        &toolbox,
        "You help find a music file on Soulseek. Plain searches for the artist and title found nothing usable. \
         Try other search words: fewer words, another spelling of the artist, the title alone, or the mix name. \
         Finish when a search finds files that look like this track, or when searches run out.",
        vec![
            Message::user(format!("Find this track. You have {MAX_SEARCHES} searches.")),
            untrusted("the track details", &wanted),
        ],
        &result,
        Limits {
            max_steps: MAX_SEARCHES + 1,
            max_result_chars: 4_000,
            max_tokens: 300,
        },
    )?;
    Ok(toolbox.found.into_inner().unwrap())
}
