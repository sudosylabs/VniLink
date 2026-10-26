use crate::clipboard::{ClipboardFileEntry, FileSelection};

fn entry(path: &str, size: u64) -> ClipboardFileEntry {
    ClipboardFileEntry {
        path: path.into(),
        size,
        dir: false,
    }
}

fn selection(entries: Vec<ClipboardFileEntry>) -> FileSelection {
    FileSelection { entries }
}

#[test]
fn file_selection_wire_codec_round_trips_and_rejects_garbage() {
    use crate::clipboard::{decode, encode, Frame};
    let offer = selection(vec![
        entry("a.txt", 3),
        entry("nested/b.bin", u64::MAX),
        ClipboardFileEntry {
            path: "empty".into(),
            size: 0,
            dir: true,
        },
    ]);
    let bytes = encode(&Frame::Files {
        revision: 7,
        selection: offer.clone(),
    })
    .unwrap();
    assert_eq!(
        decode(&bytes[4..]).unwrap(),
        Frame::Files {
            revision: 7,
            selection: offer,
        }
    );
    let empty = encode(&Frame::Files {
        revision: 3,
        selection: FileSelection::default(),
    })
    .unwrap();
    assert_eq!(
        decode(&empty[4..]).unwrap(),
        Frame::Files {
            revision: 3,
            selection: FileSelection::default(),
        }
    );
    // Tag, truncation, flag, trailing bytes and UTF-8 all fail closed.
    assert!(decode(&[]).is_err());
    assert!(decode(&[9]).is_err());
    assert!(decode(&[4, 0, 0, 0, 0, 0, 0, 0]).is_err());
    let mut truncated = bytes.clone();
    truncated.truncate(truncated.len() - 1);
    assert!(decode(&truncated[4..]).is_err());
    let mut flag = bytes.clone();
    let flag_at = 4 + 1 + 8 + 2;
    flag[flag_at] = 7;
    assert!(decode(&flag[4..]).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(decode(&trailing[4..]).is_err());
    let mut utf8 = encode(&Frame::Files {
        revision: 1,
        selection: selection(vec![entry("ok.txt", 1)]),
    })
    .unwrap();
    let name_at = utf8.len() - "ok.txt".len() - 8;
    utf8[name_at] = 0xFF;
    assert!(decode(&utf8[4..]).is_err());
    // A hostile peer cannot smuggle a jailed path or a duplicate past decode.
    let evil = selection(vec![entry("../evil", 1)]);
    assert!(encode(&Frame::Files {
        revision: 1,
        selection: evil,
    })
    .is_err());
    let dupe = selection(vec![entry("d.txt", 1), entry("d.txt", 1)]);
    assert!(encode(&Frame::Files {
        revision: 1,
        selection: dupe,
    })
    .is_err());
    // Debug never prints selected paths.
    let rendered = format!(
        "{:?}",
        Frame::Files {
            revision: 1,
            selection: selection(vec![entry("do-not-log.txt", 1)]),
        }
    );
    assert!(!rendered.contains("do-not-log"));
}

#[test]
fn offer_binding_covers_only_offered_paths() {
    let offered = selection(vec![entry("a.txt", 1), entry("nested/b.txt", 2)]);
    assert!(offered.covers(&selection(vec![entry("a.txt", 9)])));
    assert!(offered.covers(&offered.clone()));
    assert!(!offered.covers(&selection(vec![entry("evil.txt", 1)])));
    assert!(!offered.covers(&selection(vec![entry("a.txt", 1), entry("evil.txt", 1)])));
    assert!(!FileSelection::default().covers(&selection(vec![entry("a.txt", 1)])));
}
