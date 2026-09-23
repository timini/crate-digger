//! Index a folder into a Crate Digger database from the command line.
//! Useful for trying the app on a real library and for timing imports.
//!
//! cargo run --release -p cd-core --example import -- <database.sqlite> <music folder>

use std::path::{Path, PathBuf};
use std::time::Instant;

use cd_core::library::{self, AudioProbe, ProbeInfo};

struct Probe;

impl AudioProbe for Probe {
    fn probe(&self, path: &Path) -> Result<ProbeInfo, String> {
        cd_audio::probe(path)
            .map(|i| ProbeInfo {
                codec: i.codec,
                duration_ms: i.duration_ms.map(|d| d as i64),
                sample_rate: Some(i.sample_rate as i64),
                channels: Some(i.channels as i64),
            })
            .map_err(|e| e.to_string())
    }

    fn is_supported(&self, path: &Path) -> bool {
        cd_audio::decode::is_supported_extension(path)
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let db = PathBuf::from(
        args.next()
            .expect("usage: import <database.sqlite> <music folder>"),
    );
    let folder = PathBuf::from(
        args.next()
            .expect("usage: import <database.sqlite> <music folder>"),
    );
    let conn = cd_core::db::open(&db).expect("open database");
    let root = library::add_root(&conn, &folder).expect("add folder");
    let start = Instant::now();
    let summary = library::import_root(&conn, &root, &Probe, |s| {
        eprint!("\r{} / {} files", s.processed, s.total);
        Ok::<(), ()>(())
    })
    .expect("import");
    eprintln!();
    println!("{summary:#?}");
    println!("took {:?}", start.elapsed());
}
