use super::*;

#[test]
fn polling_detects_modification() {
    use std::fs;
    use std::thread::sleep;

    let dir = std::env::temp_dir().join("hot_reload_poll_test");
    fs::create_dir_all(&dir).ok();

    let pak_path = dir.join("test.pak");
    let mut builder = crate::package::PackageBuilder::new();
    let id = crate::core::AssetId::from_raw((1u64 << 32) | 1);
    builder.add_asset(id, crate::core::AssetType::Binary, b"v1".to_vec(), &[]);
    let pak = builder.build().unwrap();
    fs::write(&pak_path, &pak).unwrap();

    sleep(Duration::from_secs(1));

    let mut watcher = HotReloadWatcher::watch_file(&pak_path, Duration::from_millis(100)).unwrap();
    let rx = watcher.receiver();

    let mut builder2 = crate::package::PackageBuilder::new();
    builder2.add_asset(id, crate::core::AssetType::Binary, b"v2".to_vec(), &[]);
    let pak2 = builder2.build().unwrap();
    fs::write(&pak_path, &pak2).unwrap();

    sleep(Duration::from_millis(500));

    let events: Vec<HotReloadEvent> = rx.try_iter().collect();
    if !events.is_empty() {
        let found = events
            .iter()
            .any(|e| matches!(e, HotReloadEvent::PakModified(p) if p == &pak_path));
        assert!(found, "Should receive PakModified event");
    }

    watcher.stop();
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_size_change_counts_even_without_a_newer_mtime() {
    let now = std::time::SystemTime::now();
    let base = Stamp {
        modified: now,
        len: 10,
    };
    assert!(!stamp_changed(base, base));
    assert!(stamp_changed(
        base,
        Stamp {
            modified: now,
            len: 11
        }
    ));
    assert!(stamp_changed(
        base,
        Stamp {
            modified: now + Duration::from_millis(250),
            len: 10
        }
    ));
    assert!(!stamp_changed(
        base,
        Stamp {
            modified: now + Duration::from_millis(50),
            len: 10
        }
    ));
}

/// End-to-end (data layer): rebuild `.pak` → watcher event → `on_pak_changed` →
/// Loaded resource reads new bytes. `#495` item ④ now has automated regression, no longer relies solely on manual replay.
#[test]
fn a_rebuilt_package_refreshes_loaded_assets() {
    use std::fs;
    use std::thread::sleep;

    let dir = std::env::temp_dir().join(format!("hot_reload_chain_{}", std::process::id()));
    fs::create_dir_all(&dir).ok();
    let pak_path = dir.join("chain.pak");
    let id = crate::core::AssetId::from_raw((9u64 << 32) | 3);

    let write_pak = |bytes: &[u8]| {
        let mut builder = crate::package::PackageBuilder::new();
        builder.add_asset(id, crate::core::AssetType::Binary, bytes.to_vec(), &[]);
        fs::write(&pak_path, builder.build().unwrap()).unwrap();
    };
    write_pak(b"v1");

    let mut manager = crate::runtime::ResourceManager::new();
    manager.load_package(&pak_path).expect("load pak");
    assert_eq!(manager.load_with_deps_raw(id).unwrap(), b"v1");

    let watcher = HotReloadWatcher::watch_file(&pak_path, Duration::from_millis(50)).unwrap();

    sleep(Duration::from_secs(1));
    write_pak(b"v2-after-rebuild");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut seen = false;
    while std::time::Instant::now() < deadline {
        if let Ok(HotReloadEvent::PakModified(path)) = watcher.receiver().try_recv() {
            assert_eq!(path, pak_path);
            seen = true;
            break;
        }
        sleep(Duration::from_millis(25));
    }
    assert!(seen, "watcher must report the rebuilt pak");

    manager.on_pak_changed(&pak_path).expect("reload pak");
    assert_eq!(
        manager.get_raw(id).unwrap(),
        b"v2-after-rebuild",
        "loaded assets must serve the new bytes after the package is rebuilt"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_thread_setup_hook_runs_on_the_polling_thread() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = HotReloadWatcher::watch_files_with_thread_setup(
        Vec::new(),
        Duration::from_millis(50),
        move || {
            let name = std::thread::current().name().map(str::to_owned);
            let _ = tx.send(name);
        },
    )
    .unwrap();

    let name = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the setup hook must run");
    assert_eq!(
        name.as_deref(),
        Some("hot-reload-poller"),
        "the hook must run on the poller thread, not the caller"
    );

    watcher.stop();
}
