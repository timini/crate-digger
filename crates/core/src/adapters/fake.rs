//! In-memory adapters for tests and for running the app before real
//! integrations exist. None of them touch the network.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use super::*;

/// Lets a test make the next call(s) fail.
#[derive(Default)]
pub struct FailureSwitch(Mutex<Option<AdapterError>>);

impl FailureSwitch {
    pub fn set(&self, err: Option<AdapterError>) {
        *self.0.lock().unwrap() = err;
    }

    fn check(&self) -> AdapterResult<()> {
        match &*self.0.lock().unwrap() {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }
}

/// Returns a fixed catalogue of proposals, in order, `limit` at a time.
pub struct FakeSource {
    pub catalogue: Vec<CandidateProposal>,
    cursor: AtomicUsize,
    pub failure: FailureSwitch,
    pub requests: Mutex<Vec<DiscoveryRequest>>,
}

impl FakeSource {
    pub fn new(catalogue: Vec<CandidateProposal>) -> Self {
        FakeSource {
            catalogue,
            cursor: AtomicUsize::new(0),
            failure: FailureSwitch::default(),
            requests: Mutex::default(),
        }
    }

    /// A small built-in catalogue of invented tracks.
    pub fn demo() -> Self {
        let names = [
            (
                "Fixture Collective",
                "Night Signal",
                Some("Original Mix"),
                "Test Pressings",
            ),
            (
                "Fixture Collective",
                "Night Signal",
                Some("Dub"),
                "Test Pressings",
            ),
            ("Sine Wave Society", "Four Forty", None, "Oscillator Records"),
            (
                "The Placeholders",
                "Lorem Groove",
                Some("Extended Mix"),
                "Ipsum Audio",
            ),
            (
                "Synthetic Sound System",
                "Pure Tone",
                Some("Edit"),
                "Oscillator Records",
            ),
            ("Mock Unit", "Deterministic", None, "Seeded Sounds"),
            ("Mock Unit", "Fake Plastic Beats", Some("Remix"), "Seeded Sounds"),
            ("Null Device", "Dev Zero", None, "Kernel Cuts"),
        ];
        Self::new(
            names
                .iter()
                .map(|(artist, title, mix, label)| CandidateProposal {
                    artist: artist.to_string(),
                    title: title.to_string(),
                    mix: mix.map(str::to_string),
                    label: Some(label.to_string()),
                    release: None,
                    reasons: vec![format!("On {label}, a label in your seeds")],
                    evidence: vec![EvidenceProposal {
                        source_kind: "fake_source".into(),
                        source_url: Some(format!(
                            "https://example.invalid/tracklists/{}",
                            title.to_lowercase().replace(' ', "-")
                        )),
                        supplied_text_id: None,
                        excerpt: format!("{artist} - {title}"),
                        confidence: 0.9,
                    }],
                })
                .collect(),
        )
    }
}

impl DiscoverySource for FakeSource {
    fn id(&self) -> &str {
        "fake_source"
    }

    fn discover(&self, request: &DiscoveryRequest) -> AdapterResult<Vec<CandidateProposal>> {
        let limit = request.limit;
        self.requests.lock().unwrap().push(request.clone());
        self.failure.check()?;
        let start = self
            .cursor
            .fetch_add(limit, Ordering::SeqCst)
            .min(self.catalogue.len());
        let end = (start + limit).min(self.catalogue.len());
        Ok(self.catalogue[start..end].to_vec())
    }
}

/// Returns scripted JSON responses in order.
pub struct FakeLlm {
    responses: Mutex<Vec<serde_json::Value>>,
    pub structured: bool,
    pub requests: Mutex<Vec<StructuredRequest>>,
    pub failure: FailureSwitch,
}

impl FakeLlm {
    pub fn new(responses: Vec<serde_json::Value>) -> Self {
        FakeLlm {
            responses: Mutex::new(responses),
            structured: true,
            requests: Mutex::new(Vec::new()),
            failure: FailureSwitch::default(),
        }
    }
}

impl LlmProvider for FakeLlm {
    fn id(&self) -> &str {
        "fake_llm"
    }

    fn supports_structured_output(&self) -> bool {
        self.structured
    }

    fn complete_json(&self, req: &StructuredRequest) -> AdapterResult<serde_json::Value> {
        self.failure.check()?;
        self.requests.lock().unwrap().push(req.clone());
        let mut r = self.responses.lock().unwrap();
        if r.is_empty() {
            return Err(AdapterError::Invalid("no scripted response left".into()));
        }
        Ok(r.remove(0))
    }
}

/// "Downloads" by copying local fixture files into the destination folder.
pub struct FakeAcquirer {
    /// Files offered for every search, in preference order.
    pub library: Vec<PathBuf>,
    transfers: Mutex<HashMap<String, (String, TransferStatus)>>,
    /// Number of transfers actually started (not deduplicated calls).
    pub started: AtomicUsize,
    pub failure: FailureSwitch,
}

impl FakeAcquirer {
    pub fn new(library: Vec<PathBuf>) -> Self {
        FakeAcquirer {
            library,
            transfers: Mutex::new(HashMap::new()),
            started: AtomicUsize::new(0),
            failure: FailureSwitch::default(),
        }
    }
}

impl Acquirer for FakeAcquirer {
    fn id(&self) -> &str {
        "fake_acquirer"
    }

    fn search(&self, query: &AcquisitionQuery) -> AdapterResult<Vec<SearchResult>> {
        self.failure.check()?;
        Ok(self
            .library
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("bin");
                SearchResult {
                    result_id: i.to_string(),
                    filename: format!("{} - {}.{ext}", query.artist, query.title),
                    size_bytes: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
                    duration_ms: None,
                    format: Some(ext.to_string()),
                    bitrate_kbps: None,
                }
            })
            .collect())
    }

    fn enqueue(
        &self,
        result: &SearchResult,
        idempotency_key: &str,
        dest_dir: &Path,
    ) -> AdapterResult<String> {
        self.failure.check()?;
        let mut transfers = self.transfers.lock().unwrap();
        if let Some((id, _)) = transfers.get(idempotency_key) {
            return Ok(id.clone());
        }
        let idx: usize = result
            .result_id
            .parse()
            .map_err(|_| AdapterError::Invalid(format!("unknown result {}", result.result_id)))?;
        let src = self
            .library
            .get(idx)
            .ok_or_else(|| AdapterError::Invalid(format!("unknown result {idx}")))?;
        let dest = dest_dir.join(sanitize_fake_name(&result.filename));
        std::fs::create_dir_all(dest_dir).map_err(|e| AdapterError::Unavailable(e.to_string()))?;
        let status = match std::fs::copy(src, &dest) {
            Ok(_) => TransferStatus::Completed { path: dest },
            Err(e) => TransferStatus::Failed {
                reason: e.to_string(),
            },
        };
        self.started.fetch_add(1, Ordering::SeqCst);
        let transfer_id = crate::util::new_id();
        transfers.insert(idempotency_key.to_string(), (transfer_id.clone(), status));
        Ok(transfer_id)
    }

    fn status(&self, transfer_id: &str) -> AdapterResult<TransferStatus> {
        self.failure.check()?;
        self.transfers
            .lock()
            .unwrap()
            .values()
            .find(|(id, _)| id == transfer_id)
            .map(|(_, s)| s.clone())
            .ok_or_else(|| AdapterError::Invalid(format!("unknown transfer {transfer_id}")))
    }

    fn cancel(&self, transfer_id: &str) -> AdapterResult<()> {
        let mut transfers = self.transfers.lock().unwrap();
        for (id, status) in transfers.values_mut() {
            if id == transfer_id {
                *status = TransferStatus::Cancelled;
            }
        }
        Ok(())
    }
}

fn sanitize_fake_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || " .-_()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Records submissions keyed by idempotency key.
#[derive(Default)]
pub struct FakeCentral {
    pub accepted: Mutex<HashMap<String, (String, serde_json::Value)>>,
    pub failure: FailureSwitch,
}

impl CentralSync for FakeCentral {
    fn submit(
        &self,
        idempotency_key: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> AdapterResult<SyncAck> {
        self.failure.check()?;
        let mut accepted = self.accepted.lock().unwrap();
        let duplicate = accepted.contains_key(idempotency_key);
        if !duplicate {
            accepted.insert(idempotency_key.to_string(), (kind.to_string(), payload.clone()));
        }
        Ok(SyncAck {
            idempotency_key: idempotency_key.to_string(),
            duplicate,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_acquirer_deduplicates_by_idempotency_key() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.wav");
        std::fs::write(&src, b"RIFF").unwrap();
        let acq = FakeAcquirer::new(vec![src]);
        let q = AcquisitionQuery {
            artist: "A".into(),
            title: "B".into(),
            mix: None,
        };
        let r = acq.search(&q).unwrap().remove(0);
        let dest = dir.path().join("staging");
        let t1 = acq.enqueue(&r, "key-1", &dest).unwrap();
        let t2 = acq.enqueue(&r, "key-1", &dest).unwrap();
        assert_eq!(t1, t2);
        assert_eq!(acq.started.load(Ordering::SeqCst), 1);
        assert!(matches!(
            acq.status(&t1).unwrap(),
            TransferStatus::Completed { .. }
        ));
    }

    #[test]
    fn fake_source_pages_through_catalogue_and_can_fail() {
        let src = FakeSource::demo();
        let request = DiscoveryRequest {
            seeds: vec![],
            limit: 3,
            input: DiscoveryInput::Seeds,
        };
        assert_eq!(src.discover(&request).unwrap().len(), 3);
        src.failure.set(Some(AdapterError::Unavailable("down".into())));
        assert!(src.discover(&request).is_err());
    }

    #[test]
    fn fake_central_reports_duplicates() {
        let c = FakeCentral::default();
        let p = serde_json::json!({"a": 1});
        assert!(!c.submit("k", "metadata", &p).unwrap().duplicate);
        assert!(c.submit("k", "metadata", &p).unwrap().duplicate);
        assert_eq!(c.accepted.lock().unwrap().len(), 1);
    }
}
