//! Index a folder into a Crate Digger database from the command line.
//! Useful for trying the app on a real library and for timing imports.
//!
//! cargo run --release -p cd-core --example import -- <database.sqlite> <music folder>

use std::path::PathBuf;
use std::time::Instant;

use cd_core::library;

#[path = "../test_support/real_probe.rs"]
mod real_probe;

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
    let summary = library::import_root(&conn, &root, &real_probe::RealProbe, |s| {
        eprint!("\r{} / {} files", s.processed, s.total);
        Ok::<(), ()>(())
    })
    .expect("import");
    eprintln!();
    println!("{summary:#?}");
    println!("took {:?}", start.elapsed());
}
