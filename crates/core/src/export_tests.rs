use quick_xml::events::Event;
use quick_xml::Reader;

use super::*;
use crate::domain::Field;

fn track(id: &str, path: &str, title: &str) -> ExportTrack {
    ExportTrack {
        position: 0,
        track_id: id.into(),
        path: path.into(),
        meta: TrackMeta {
            artist: Some("Alpha & Beta".into()),
            title: Some(title.into()),
            mix: Some("Dub \"Mix\"".into()),
            label: Some("Label <One>".into()),
            release: None,
            track_number: None,
            year: Some(2020),
            genre: None,
            tempo: Some(127.9),
            musical_key: Some("8A".into()),
        },
        duration_ms: Some(301_400),
        size_bytes: 1234,
        format: Some("flac".into()),
        tempo: Some(128.0),
        key: Some("Am".into()),
    }
}

#[test]
fn file_uris_match_rekordbox() {
    assert_eq!(
        file_uri("/Users/dj/Music/Alpha & Beta/01 Café #1 100%.flac"),
        "file://localhost/Users/dj/Music/Alpha%20%26%20Beta/01%20Caf%C3%A9%20%231%20100%25.flac"
    );
    assert_eq!(
        file_uri(r"C:\Users\DJ\Music\track one.mp3"),
        "file://localhost/C:/Users/DJ/Music/track%20one.mp3"
    );
}

#[test]
fn m3u8_keeps_order_and_single_lines() {
    let tracks = vec![
        track("a", "/m/One.flac", "One"),
        track("b", "/m/Two\nlines.flac", "Two"),
    ];
    let out = m3u8(&tracks);
    assert_eq!(
        out,
        "#EXTM3U\n#EXTINF:301,Alpha & Beta - One (Dub \"Mix\")\n/m/One.flac\n#EXTINF:301,Alpha & Beta - Two (Dub \"Mix\")\n/m/Two lines.flac\n"
    );
}

/// (collection tracks as (id, name, location), playlist keys)
fn parse(xml: &str) -> (Vec<(String, String, String)>, Vec<String>) {
    let mut reader = Reader::from_str(xml);
    let (mut collection, mut keys) = (vec![], vec![]);
    let mut in_playlists = false;
    loop {
        match reader.read_event().unwrap() {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.name().as_ref().as_bytes().to_vec();
                if name == b"PLAYLISTS" {
                    in_playlists = true;
                }
                if name == b"TRACK" {
                    let get = |k: &[u8]| {
                        e.attributes()
                            .flatten()
                            .find(|a| a.key.as_ref().as_bytes() == k)
                            .map(|a| quick_xml::escape::unescape(&a.value).unwrap().to_string())
                            .unwrap_or_default()
                    };
                    if in_playlists {
                        keys.push(get(b"Key"));
                    } else {
                        collection.push((get(b"TrackID"), get(b"Name"), get(b"Location")));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    (collection, keys)
}

#[test]
fn rekordbox_xml_is_valid_and_keeps_order_and_repeats() {
    let tracks = vec![
        track("a", "/m/One.flac", "One & <Only>"),
        track("b", "/m/Two.flac", "Two"),
        track("a", "/m/One.flac", "One & <Only>"),
    ];
    let xml = rekordbox_xml("Friday's \"set\"", &tracks);
    let (collection, keys) = parse(&xml);
    assert_eq!(
        collection,
        vec![
            (
                "1".into(),
                "One & <Only>".into(),
                "file://localhost/m/One.flac".into()
            ),
            ("2".into(), "Two".into(), "file://localhost/m/Two.flac".into()),
        ]
    );
    assert_eq!(keys, vec!["1", "2", "1"]);
    assert!(xml.contains("Name=\"Friday&apos;s &quot;set&quot;\" Type=\"1\" KeyType=\"0\" Entries=\"3\""));
    assert!(xml.contains("AverageBpm=\"128.00\"") && xml.contains("Tonality=\"Am\""));
    // No cue points or beat grids.
    assert!(!xml.contains("POSITION_MARK") && !xml.contains("<TEMPO"));
}

#[test]
fn prepare_reports_problems_and_ignores_estimated_tempo_and_key() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(&dir.path().join("db.sqlite")).unwrap();
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../audio/tests/fixtures/tone.flac");
    let kept = dir.path().join("kept.flac");
    std::fs::copy(&fixture, &kept).unwrap();
    let staged = dir.path().join("staged.flac");
    std::fs::copy(&fixture, &staged).unwrap();
    let probe = crate::real_probe::RealProbe;

    let mut ids = vec![];
    for (name, path) in [("Kept", Some(&kept)), ("Staged", Some(&staged)), ("Gone", None)] {
        let t = meta::create_track(&conn).unwrap();
        meta::set_extracted(&conn, &t, "tags", &[(Field::Title, Some(name.into()))]).unwrap();
        if let Some(p) = path {
            let r = library::register_staged(&conn, p, &t, &probe).unwrap();
            if name == "Kept" {
                conn.execute(
                    "UPDATE audio_file SET origin = 'archived' WHERE id = ?1",
                    [&r.file_id],
                )
                .unwrap();
            }
        }
        ids.push(t);
    }
    // Analysis estimated the kept track's tempo and key; the tag gives a key.
    meta::set_extracted(
        &conn,
        &ids[0],
        "analysis",
        &[
            (Field::Tempo, Some("123.4".into())),
            (Field::MusicalKey, Some("5A".into())),
        ],
    )
    .unwrap();
    meta::set_extracted(&conn, &ids[0], "tags", &[(Field::MusicalKey, Some("Cm".into()))]).unwrap();
    // The user's correction counts even over analysis.
    meta::set_correction(&conn, &ids[1], Field::Tempo, Some("126")).unwrap();

    let p = playlists::create(&conn, "Set").unwrap();
    playlists::add_tracks(&conn, &p, &ids, None).unwrap();
    let prepared = prepare(&conn, &p).unwrap();
    assert_eq!(prepared.name, "Set");
    assert_eq!(prepared.tracks.len(), 2);
    match prepared.problems.as_slice() {
        [Problem::Temporary {
            position: 1,
            track: staged,
        }, Problem::Missing {
            position: 2,
            track: gone,
        }] => {
            assert!(staged.contains("Staged"), "{staged}");
            assert_eq!(gone, "Gone");
        }
        other => panic!("{other:?}"),
    }
    let kept_track = &prepared.tracks[0];
    assert_eq!(kept_track.tempo, None, "estimated tempo is not exported");
    assert_eq!(kept_track.key.as_deref(), Some("Cm"));
    assert_eq!(prepared.tracks[1].tempo, Some(126.0));
}
