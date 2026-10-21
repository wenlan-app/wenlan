// SPDX-License-Identifier: AGPL-3.0-only
use super::*;

fn fixture() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store {
        directory: dir.path().join("relay-v1"),
    };
    (dir, store)
}
fn device() -> DeviceCredential {
    DeviceCredential {
        id: "d".repeat(64),
        management_token: "m".repeat(64),
        expires_at: super::super::now_ms() + 60_000,
    }
}
fn enrolled(store: &Store) -> Profile {
    let profile = store.configure(None, "review").unwrap();
    let enabled = store.enable(profile.revision()).unwrap();
    store.attach_device(enabled.revision(), device()).unwrap()
}

#[test]
fn missing_store_does_not_create_data_or_import_legacy_identity() {
    let (dir, store) = fixture();
    std::fs::write(dir.path().join("relay_id"), "uold-secret").unwrap();
    assert!(store.load().unwrap().is_none());
    assert!(!store.directory.exists());
    let profile = store.configure(None, "review").unwrap();
    assert!(!profile.enabled());
    assert!(profile.device().is_none());
    assert!(!profile.backend_token().contains("old-secret"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("relay_id")).unwrap(),
        "uold-secret"
    );
}

#[test]
fn native_profile_survives_reload_but_frontend_view_and_debug_exclude_secrets() {
    let (_dir, store) = fixture();
    let profile = enrolled(&store);
    let restored = Store {
        directory: store.directory.clone(),
    }
    .load()
    .unwrap()
    .unwrap();
    assert_eq!(restored.revision(), profile.revision());
    assert_eq!(restored.backend_token(), profile.backend_token());
    assert_eq!(
        restored.device().unwrap().management_token,
        device().management_token
    );
    let public = serde_json::to_string(&restored.view()).unwrap();
    let debug = format!("{restored:?}");
    for value in [
        profile.backend_token(),
        &device().id,
        &device().management_token,
        RELAY_ORIGIN,
    ] {
        assert!(!public.contains(value));
        assert!(!debug.contains(value));
    }
    assert_eq!(restored.view().space, "review");
}

#[test]
fn disable_persists_before_revocation_and_old_callbacks_cannot_restore_access() {
    let (_dir, store) = fixture();
    let active = enrolled(&store);
    let off = store.disable(active.revision()).unwrap();
    assert!(!store.load().unwrap().unwrap().enabled());
    assert!(off.view().disconnect_pending);
    assert_eq!(
        store
            .attach_device(active.revision(), device())
            .unwrap_err(),
        StoreError::Stale
    );
    assert_eq!(
        store.attach_device(off.revision(), device()).unwrap_err(),
        StoreError::Stale
    );
    assert_eq!(
        store.enable(off.revision()).unwrap_err(),
        StoreError::DisconnectPending
    );
    assert_eq!(
        store
            .configure(Some(off.revision()), "private")
            .unwrap_err(),
        StoreError::DisconnectPending
    );
    assert_eq!(
        store
            .finish_disconnect(off.revision(), &"x".repeat(64))
            .unwrap_err(),
        StoreError::Stale
    );
    let cleared = store
        .finish_disconnect(off.revision(), &device().id)
        .unwrap();
    assert!(cleared.device().is_none());
    assert_ne!(cleared.backend_token(), active.backend_token());
    let changed = store.configure(Some(cleared.revision()), "next").unwrap();
    let next = store.enable(changed.revision()).unwrap();
    assert_eq!(next.space(), "next");
    assert_eq!(
        store
            .finish_disconnect(off.revision(), &device().id)
            .unwrap_err(),
        StoreError::Stale
    );
    assert!(store.load().unwrap().unwrap().enabled());
}

#[test]
fn stale_selection_and_cross_device_rotation_are_rejected_without_overwriting_state() {
    let (_dir, store) = fixture();
    let active = enrolled(&store);
    assert_eq!(
        store.configure(None, "private").unwrap_err(),
        StoreError::Stale
    );
    let mut replacement = device();
    replacement.id = "x".repeat(64);
    assert_eq!(
        store
            .attach_device(active.revision(), replacement)
            .unwrap_err(),
        StoreError::Stale
    );
    assert_eq!(store.load().unwrap().unwrap().revision(), active.revision());
}

#[test]
fn write_is_off_by_default_and_a_profile_saved_before_it_existed_still_loads() {
    let (_dir, store) = fixture();
    let active = enrolled(&store);
    assert!(!active.write_enabled());
    // Not written while false, so an older build reads the same file.
    let json = serde_json::to_value(&active).unwrap();
    assert!(json.get("write_enabled").is_none());
    let legacy: Profile = serde_json::from_value(json).unwrap();
    assert!(!legacy.write_enabled());
}

#[test]
fn enable_write_keeps_the_revision_and_needs_the_current_enrolled_profile() {
    let (_dir, store) = fixture();
    let active = enrolled(&store);
    assert_eq!(
        store.enable_write("stale-revision").unwrap_err(),
        StoreError::Stale
    );
    assert!(!store.load().unwrap().unwrap().write_enabled());
    let written = store.enable_write(active.revision()).unwrap();
    assert!(written.write_enabled());
    assert_eq!(written.revision(), active.revision());
    let reloaded = store.load().unwrap().unwrap();
    assert!(reloaded.write_enabled());
    assert_eq!(reloaded.revision(), active.revision());
    // Asking again changes nothing.
    assert!(store
        .enable_write(active.revision())
        .unwrap()
        .write_enabled());

    // A profile that is not on, or has no device yet, cannot be written to.
    let (_dir, other) = fixture();
    let configured = other.configure(None, "review").unwrap();
    assert_eq!(
        other.enable_write(configured.revision()).unwrap_err(),
        StoreError::Stale
    );
    let enabled = other.enable(configured.revision()).unwrap();
    assert_eq!(
        other.enable_write(enabled.revision()).unwrap_err(),
        StoreError::Stale
    );
}

#[test]
fn write_ends_whenever_the_grants_end() {
    // Turning off, then the disconnect finishing.
    let (_dir, store) = fixture();
    let active = enrolled(&store);
    store.enable_write(active.revision()).unwrap();
    let off = store.disable(active.revision()).unwrap();
    assert!(!off.write_enabled());
    assert!(!store.load().unwrap().unwrap().write_enabled());
    let finished = store
        .finish_disconnect(off.revision(), &device().id)
        .unwrap();
    assert!(!finished.write_enabled());

    // Turning it on again starts from search only.
    let again = store.enable(finished.revision()).unwrap();
    assert!(!again.write_enabled());

    // Renewing the key (a same device rotation) ends every grant too.
    let (_dir, store) = fixture();
    let active = enrolled(&store);
    store.enable_write(active.revision()).unwrap();
    let mut rotated = device();
    rotated.management_token = "n".repeat(64);
    let renewed = store.attach_device(active.revision(), rotated).unwrap();
    assert!(!renewed.write_enabled());
    assert!(!store.load().unwrap().unwrap().write_enabled());

    // Choosing a different Space is a new profile, which starts from search only.
    let (_dir, store) = fixture();
    let active = enrolled(&store);
    store.enable_write(active.revision()).unwrap();
    let off = store.disable(active.revision()).unwrap();
    let finished = store
        .finish_disconnect(off.revision(), &device().id)
        .unwrap();
    let other_space = store
        .configure(Some(finished.revision()), "private")
        .unwrap();
    assert!(!other_space.write_enabled());
}

#[test]
fn concurrent_creators_have_one_winner_and_never_share_a_partial_file() {
    let (_dir, store) = fixture();
    prepare_directory(&store.directory).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let jobs: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let directory = store.directory.clone();
            std::thread::spawn(move || {
                barrier.wait();
                Store { directory }.configure(None, "review")
            })
        })
        .collect();
    let results: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(results
        .iter()
        .filter_map(|result| result.as_ref().err())
        .all(|error| matches!(error, StoreError::Stale | StoreError::Busy)));
    assert!(store.load().unwrap().is_some());
}

#[test]
fn lock_contention_is_bounded_and_does_not_modify_the_profile() {
    let (_dir, store) = fixture();
    let original = store.configure(None, "review").unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.directory.join("connection.lock"))
        .unwrap();
    lock.lock().unwrap();
    #[cfg(unix)]
    let inherited = lock.try_clone().unwrap();
    assert_eq!(
        store.enable(original.revision()).unwrap_err(),
        StoreError::Busy
    );
    lock.unlock().unwrap();
    drop(lock);
    assert_eq!(
        store.load().unwrap().unwrap().revision(),
        original.revision()
    );
    #[cfg(unix)]
    drop(inherited);
}

#[test]
fn invalid_write_or_corrupt_file_does_not_reset_to_an_enabled_default() {
    let (_dir, store) = fixture();
    let profile = store.configure(None, "review").unwrap();
    let path = store.directory.join(FILE_NAME);
    let before = std::fs::read(&path).unwrap();
    let mut invalid = profile.clone();
    invalid.relay_origin = "https://other.example".into();
    assert_eq!(
        write_profile(&path, &invalid).unwrap_err(),
        StoreError::Invalid
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::write(&path, b"PRIVATE_CORRUPT_DATA").unwrap();
    let error = store.load().unwrap_err();
    assert!(!error.to_string().contains("PRIVATE_CORRUPT_DATA"));
    assert!(store.configure(None, "review").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"PRIVATE_CORRUPT_DATA");
}

#[test]
fn expired_credentials_remain_available_for_explicit_disconnect_recovery() {
    let (_dir, store) = fixture();
    let mut profile = enrolled(&store);
    profile.enabled = false;
    profile.device.as_mut().unwrap().expires_at = 1;
    write_profile(&store.directory.join(FILE_NAME), &profile).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert!(loaded.view().disconnect_pending);
    assert_eq!(loaded.view().credential_expires_at, Some(1));
    assert_eq!(
        store.enable(loaded.revision()).unwrap_err(),
        StoreError::DisconnectPending
    );
}

#[test]
fn whole_library_profile_survives_reload() {
    let (_dir, store) = fixture();
    let profile = store
        .configure(None, super::super::WHOLE_LIBRARY_SPACE)
        .unwrap();
    assert!(profile.whole_library());
    let enabled = store.enable(profile.revision()).unwrap();
    let reloaded = store.load().unwrap().unwrap();
    assert_eq!(reloaded.revision(), enabled.revision());
    assert!(reloaded.whole_library());
    assert_eq!(reloaded.view().space, "*");

    let (_dir, store) = fixture();
    assert!(!store.configure(None, "review").unwrap().whole_library());
}

#[test]
fn oversized_storage_and_invalid_spaces_fail_closed() {
    let (_dir, store) = fixture();
    for space in ["", " review", "review\n", &"x".repeat(257)] {
        assert_eq!(
            store.configure(None, space).unwrap_err(),
            StoreError::Invalid
        );
    }
    store.configure(None, "review").unwrap();
    std::fs::write(
        store.directory.join(FILE_NAME),
        vec![b'x'; MAX_ENCODED_BYTES + 1],
    )
    .unwrap();
    assert_eq!(store.load().unwrap_err(), StoreError::Invalid);
}

#[cfg(unix)]
#[test]
fn unix_storage_is_private_and_symlinks_are_not_followed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (dir, store) = fixture();
    store.configure(None, "review").unwrap();
    assert_eq!(
        std::fs::metadata(&store.directory)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for name in [FILE_NAME, "connection.lock"] {
        assert_eq!(
            std::fs::metadata(store.directory.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let outside = dir.path().join("outside");
    std::fs::write(&outside, "unchanged").unwrap();
    std::fs::remove_file(store.directory.join(FILE_NAME)).unwrap();
    symlink(&outside, store.directory.join(FILE_NAME)).unwrap();
    assert_eq!(store.load().unwrap_err(), StoreError::Storage);
    assert_eq!(
        store.configure(None, "review").unwrap_err(),
        StoreError::Storage
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "unchanged");
    let linked = Store {
        directory: dir.path().join("linked"),
    };
    symlink(&store.directory, &linked.directory).unwrap();
    assert_eq!(linked.load().unwrap_err(), StoreError::Storage);
}

#[test]
fn failed_operation_releases_the_lock_for_the_next_call() {
    let (_dir, store) = fixture();
    let profile = store.configure(None, "review").unwrap();
    // A returned error (Stale) must not leave the lock held: the next call
    // on the production Store path succeeds without Busy.
    assert_eq!(
        store.configure(None, "review").unwrap_err(),
        StoreError::Stale
    );
    assert_eq!(
        store.configure(None, "other").unwrap_err(),
        StoreError::Stale
    );
    let loaded = store.load().unwrap().unwrap();
    assert_eq!(loaded.revision(), profile.revision());
    store.enable(profile.revision()).unwrap();
}

#[test]
fn lock_guard_releases_on_panic_unwind() {
    let (_dir, store) = fixture();
    prepare_directory(&store.directory).unwrap();
    let lock_path = store.directory.join("connection.lock");
    // The guard lives inside the unwinding closure, so its Drop (and the
    // explicit unlock) runs during panic unwind. The duplicated handle is
    // stashed outside to prove the lock is free while it stays open.
    let saved: std::cell::RefCell<Option<File>> = std::cell::RefCell::new(None);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        lock.try_lock().unwrap();
        let guard = LockGuard::hold(lock);
        saved.borrow_mut().replace(guard.file.try_clone().unwrap());
        panic!("simulated operation panic");
    }));
    assert!(outcome.is_err());
    let inherited = saved.borrow_mut().take().unwrap();
    // The explicit unlock in Drop releases the lock even though the
    // duplicated `inherited` description is still open.
    let contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    contender.try_lock().unwrap();
    contender.unlock().unwrap();
    drop(inherited);
}

#[cfg(unix)]
#[test]
fn lock_guard_releases_despite_duplicated_description_but_holds_while_alive() {
    let (_dir, store) = fixture();
    prepare_directory(&store.directory).unwrap();
    let lock_path = store.directory.join("connection.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    lock.try_lock().unwrap();
    let guard = LockGuard::hold(lock);
    // try_clone duplicates the open file description, modelling a
    // fork/spawn-inherited handle without raw fork in the test runner.
    let inherited = guard.file.try_clone().unwrap();
    // While the guard is alive the lock is contended: no second owner.
    let contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    assert!(matches!(
        contender.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(guard);
    // After the guard's explicit unlock, the lock is free even though the
    // duplicated `inherited` handle is still open. Close-only release would
    // still report Busy here.
    let contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    contender.try_lock().unwrap();
    contender.unlock().unwrap();
    drop(inherited);
}

#[cfg(windows)]
#[test]
fn windows_dpapi_round_trip_stores_no_plaintext_and_has_no_plaintext_fallback() {
    let (_dir, store) = fixture();
    let profile = enrolled(&store);
    let encoded = std::fs::read(store.directory.join(FILE_NAME)).unwrap();
    assert!(encoded.starts_with(b"WENLAN-DPAPI-1\n"));
    for secret in [profile.backend_token(), &device().management_token] {
        assert!(!encoded
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
    }
    assert_eq!(
        store
            .load()
            .unwrap()
            .unwrap()
            .device()
            .unwrap()
            .management_token,
        device().management_token
    );
    assert_eq!(
        unprotect(&serde_json::to_vec(&profile).unwrap()).unwrap_err(),
        StoreError::Protection
    );
}

#[test]
fn a_brief_lock_is_waited_out_and_a_held_one_reports_busy() {
    let (_dir, store) = fixture();
    let original = store.configure(None, "review").unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.directory.join("connection.lock"))
        .unwrap();
    lock.lock().unwrap();
    #[cfg(unix)]
    let inherited = lock.try_clone().unwrap();
    assert_eq!(store.load().unwrap_err(), StoreError::Busy);

    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(60));
        lock.unlock().unwrap();
    });
    assert_eq!(
        store.load().unwrap().unwrap().revision(),
        original.revision()
    );
    release.join().unwrap();
    #[cfg(unix)]
    drop(inherited);
}
