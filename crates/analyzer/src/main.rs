//! `cd-analyzer`: the analysis worker as its own binary, for development
//! and tests. The app runs the same code by starting itself with
//! `--analysis-worker`. CD_ANALYZER_FAULT injects failures for tests:
//! crash, hang, garbage, memory.

fn main() {
    cd_analyzer::worker_main();
}
