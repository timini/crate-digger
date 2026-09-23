//! Calibration of the automatic download rule against labelled result sets
//! (tests/acquisition_cases.json). Fails if the rule ever starts a download
//! of a result that is not the wanted recording, or if a decision differs
//! from the label.
//!
//! Set CALIBRATION_REPORT=<path> to write the results as Markdown.

use cd_core::acquisition::matching::{assess, Decision};
use cd_core::adapters::{AcquisitionQuery, SearchResult};
use serde::Deserialize;

#[derive(Deserialize)]
struct Cases {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    query: (String, String, Option<String>),
    expect: String,
    results: Vec<Result>,
}

#[derive(Deserialize)]
struct Result {
    path: String,
    format: String,
    kbps: Option<u32>,
    secs: Option<u64>,
    right: bool,
}

#[test]
fn automatic_downloads_never_pick_a_wrong_file() {
    let cases: Cases = serde_json::from_str(include_str!("acquisition_cases.json")).unwrap();
    let mut rows = vec![];
    let (mut wrong_auto, mut mismatched) = (vec![], vec![]);
    let (mut auto, mut choose, mut nothing, mut recommended_right, mut recommended) = (0, 0, 0, 0, 0);
    for case in &cases.cases {
        let query = AcquisitionQuery {
            artist: case.query.0.clone(),
            title: case.query.1.clone(),
            mix: case.query.2.clone(),
        };
        let results: Vec<SearchResult> = case
            .results
            .iter()
            .map(|r| SearchResult {
                result_id: r.path.clone(),
                filename: r.path.clone(),
                size_bytes: 1,
                duration_ms: r.secs.map(|s| s * 1000),
                format: Some(r.format.clone()),
                bitrate_kbps: r.kbps,
                ..Default::default()
            })
            .collect();
        let right = |path: &str| case.results.iter().any(|r| r.right && r.path == path);
        let outcome = assess(&query, &results);
        let (got, detail) = match &outcome.decision {
            Decision::Auto => {
                auto += 1;
                let pick = &outcome.ranked[0].result.filename;
                if !right(pick) {
                    wrong_auto.push(case.name.clone());
                }
                (
                    "auto",
                    format!("downloads {}", pick.rsplit('\\').next().unwrap_or(pick)),
                )
            }
            Decision::Choose {
                recommended: rec,
                why,
            } => {
                choose += 1;
                if let Some(i) = rec {
                    recommended += 1;
                    if right(&outcome.ranked[*i].result.filename) {
                        recommended_right += 1;
                    }
                }
                ("choose", why.clone())
            }
            Decision::Nothing { why } => {
                nothing += 1;
                ("nothing", why.clone())
            }
        };
        if got != case.expect {
            mismatched.push(format!("{}: expected {}, got {got}", case.name, case.expect));
        }
        rows.push(format!(
            "| {} | {} | {got} | {} |",
            case.name,
            case.expect,
            detail.replace('|', "/")
        ));
    }
    if let Ok(path) = std::env::var("CALIBRATION_REPORT") {
        let mut md = String::from("| Case | Expected | Result | Detail |\n| --- | --- | --- | --- |\n");
        md.push_str(&rows.join("\n"));
        md.push_str(&format!(
            "\n\n{} cases: {auto} automatic, {choose} sent to the user, {nothing} with nothing usable. \
             Wrong automatic downloads: {}. Recommendations that were the right file: {recommended_right} of {recommended}.\n",
            cases.cases.len(),
            wrong_auto.len()
        ));
        std::fs::write(path, md).unwrap();
    }
    assert!(wrong_auto.is_empty(), "wrong automatic downloads: {wrong_auto:?}");
    assert!(mismatched.is_empty(), "{mismatched:#?}");
}
