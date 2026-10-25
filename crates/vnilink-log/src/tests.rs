use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use super::filter::Filters;
use super::recorder::{Recorder, Ring, RING_BYTES, RING_EVENTS};
use super::*;

fn scratch(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("vnilink-log-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    folder
}

#[test]
fn a_ring_drops_its_oldest_past_its_bounds() {
    let mut ring = Ring::default();
    for index in 0..RING_EVENTS + 10 {
        ring.push(index as u64, format!("{index}"));
    }
    assert_eq!(ring.events.len(), RING_EVENTS);
    assert_eq!(
        ring.events.front().map(|(_, line)| line.as_str()),
        Some("10")
    );
    let mut ring = Ring::default();
    let big = "x".repeat(RING_BYTES / 2 + 1);
    ring.push(0, big.clone());
    ring.push(1, big);
    assert_eq!(ring.events.len(), 1);
    assert!(ring.bytes <= RING_BYTES);
}

#[test]
fn services_are_kept_apart_and_exported_in_time_order() {
    let mut recorder = Recorder::default();
    recorder.push(Some(7), "a".into(), 1);
    recorder.push(None, "b".into(), 2);
    recorder.push(Some(7), "c".into(), 3);
    for service in 100..120 {
        recorder.push(Some(service), "busy".into(), 4);
    }
    assert_eq!(recorder.services.len(), 16);
    assert!(
        !recorder.services.contains_key(&7),
        "the quietest service makes room"
    );
    let mut recorder = Recorder::default();
    recorder.push(Some(7), "a".into(), 1);
    recorder.push(None, "b".into(), 2);
    recorder.push(Some(7), "c".into(), 3);
    assert_eq!(recorder.export(), "a\nb\nc\n");
}

#[test]
fn an_export_past_its_bound_keeps_the_newest_and_says_so() {
    let mut recorder = Recorder::default();
    let line = "x".repeat(1024 * 1024);
    for at in 0..6 {
        recorder.push(Some(at), format!("{at}{line}"), at);
    }
    let exported = recorder.export();
    assert!(exported.len() <= MAX_EXPORT_BYTES + 64);
    assert!(
        exported.starts_with("{\"truncated\":3}\n"),
        "{}",
        &exported[..20]
    );
    assert!(exported
        .lines()
        .last()
        .is_some_and(|last| last.starts_with('5')));
}

#[test]
fn the_environment_wins_over_the_setting_and_a_bad_one_is_refused() {
    let mut filters = Filters::from_environment(None);
    assert_eq!(filters.state().source, FilterSource::Default);
    assert_eq!(filters.state().directives, DEFAULT_FILTER);
    filters.set(Some("vnilink::files=debug")).unwrap();
    assert_eq!(
        filters.state(),
        FilterState {
            directives: "vnilink::files=debug".into(),
            source: FilterSource::Setting
        }
    );
    assert_eq!(filters.set(Some("vnilink=loud")), Err(InvalidFilter));
    assert_eq!(
        filters.set(Some(&"a".repeat(MAX_FILTER_BYTES + 1))),
        Err(InvalidFilter)
    );
    assert_eq!(
        filters.state().source,
        FilterSource::Setting,
        "a refused filter changes nothing"
    );
    filters.set(Some("  ")).unwrap();
    assert_eq!(filters.state().source, FilterSource::Default);

    let mut filters = Filters::from_environment(Some("vnilink=trace"));
    filters.set(Some("off")).unwrap();
    assert_eq!(filters.state().source, FilterSource::Environment);
    assert_eq!(filters.state().directives, "vnilink=trace");

    let filters = Filters::from_environment(Some("vnilink=loud"));
    assert!(filters.rejected_environment());
    assert_eq!(filters.state().source, FilterSource::Default);
}

#[test]
fn runs_are_named_by_their_utc_start_and_the_oldest_pruned() {
    let at = UNIX_EPOCH + Duration::from_secs(1_793_016_245);
    assert_eq!(file::run_name(at), "20261026T120405");
    assert_eq!(file::run_name(UNIX_EPOCH), "19700101T000000");
    let root = scratch("prune");
    std::fs::create_dir_all(root.join("not-a-run")).unwrap();
    for day in 10..22 {
        std::fs::create_dir_all(root.join(format!("202610{day}T000000"))).unwrap();
    }
    file::prune(&root, 10).unwrap();
    let mut left: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left.len(), 11);
    assert_eq!(left[0], "20261012T000000");
    assert!(left.contains(&"not-a-run".to_owned()));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_tail_starts_on_a_whole_line() {
    let root = scratch("tail");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("engine.log");
    std::fs::write(&path, "first line\nsecond\nthird\n").unwrap();
    assert_eq!(file::tail(&path, 13).unwrap(), "third\n");
    assert_eq!(
        file::tail(&path, 1000).unwrap(),
        "first line\nsecond\nthird\n"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The process-wide log, in one test since its filter is shared.
#[test]
fn the_installed_log_filters_writes_records_and_changes_while_running() {
    install();
    let root = scratch("installed");
    let path = write_to(&root).unwrap();
    assert_eq!(path.file_name().unwrap(), "engine.log");
    assert_eq!(
        write_to(&scratch("again")).unwrap(),
        path,
        "one file per process"
    );

    set_filter(Some("vnilink::files=debug")).unwrap();
    tracing::debug!(target: "vnilink::files", job = 3u64, "chunk written");
    tracing::info!(target: "vnilink::camera", "camera opened");
    let written = tail(1 << 20).unwrap();
    assert!(written.contains("chunk written"), "{written}");
    assert!(written.contains("\"job\":3"));
    assert!(!written.contains("camera opened"), "{written}");
    // The recorder keeps VniLink's debug events whatever the output filter.
    assert!(export().contains("camera opened"));

    tracing::warn!(target: "iroh::socket", "endpoint at 192.0.2.1");
    set_filter(None).unwrap();
    tracing::warn!(target: "iroh::socket", "endpoint at 192.0.2.2");
    tracing::debug!(target: "iroh::socket", "endpoint at 192.0.2.3");
    let written = tail(1 << 20).unwrap();
    assert!(!written.contains("192.0.2.1"), "outside the filter");
    assert!(
        written.contains("192.0.2.2"),
        "a library's warning reaches the file"
    );
    assert!(
        !written.contains("192.0.2.3"),
        "a library's debug never does"
    );
    assert!(
        !export().contains("192.0.2."),
        "a library's events are never recorded"
    );

    tracing::info!(target: "vnilink::session", service = 4242u64, "first");
    tracing::debug!(target: "vnilink::still", service = 4242u64, budget = 9u64, "second");
    let exported = export();
    let ours: Vec<&str> = exported
        .lines()
        .filter(|line| line.contains("\"service\":4242"))
        .collect();
    assert_eq!(ours.len(), 2, "{exported}");
    assert!(ours[0].contains("first") && ours[1].contains("\"budget\":9"));

    set_filter(Some("off")).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let recorded = export();
    tracing::error!(target: "vnilink::files", "after off");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert_eq!(export(), recorded);
    assert_eq!(filter().directives, "off");

    assert_eq!(set_filter(Some("vnilink=loud")), Err(InvalidFilter));
    assert_eq!(
        filter().directives,
        "off",
        "a refused filter keeps the one in force"
    );
    set_filter(None).unwrap();
    let _ = std::fs::remove_dir_all(root);
}
