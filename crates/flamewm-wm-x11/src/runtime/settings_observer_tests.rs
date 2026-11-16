use super::*;

#[test]
fn publish_settings_snapshot_notifies_observer_with_same_snapshot() {
    let seen: Rc<RefCell<Vec<SettingsSnapshot>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    let boxed: SettingsObserver = Box::new(move |snapshot: &SettingsSnapshot| {
        sink.borrow_mut().push(snapshot.clone());
    });
    let observer: SharedSettingsObserver = Rc::new(RefCell::new(Some(boxed)));
    let mut snapshot = SettingsSnapshot::new(7);
    snapshot.values.insert(
        "appearance".to_owned(),
        SettingValue::Text("light".to_owned()),
    );
    publish_settings_snapshot(&observer, &snapshot);
    assert_eq!(seen.borrow().as_slice(), std::slice::from_ref(&snapshot));
}

#[test]
fn publish_settings_snapshot_without_observer_is_a_noop() {
    let observer: SharedSettingsObserver = Rc::new(RefCell::new(None));
    publish_settings_snapshot(&observer, &SettingsSnapshot::new(1));
}

fn readable() -> FdReadiness {
    FdReadiness {
        readable: true,
        ..FdReadiness::default()
    }
}

#[test]
fn settings_wake_clean_drain_keeps_registration() {
    let mut drained = 0;
    let action = settings_wake_step(readable(), || {
        drained += 1;
        Ok(())
    });
    assert_eq!(action, FdAction::Continue);
    assert_eq!(drained, 1);
}

#[test]
fn settings_wake_drain_error_removes_registration() {
    let action = settings_wake_step(readable(), || Err("drain failed".to_owned()));
    assert_eq!(action, FdAction::Remove);
}

#[test]
fn settings_wake_hangup_or_error_readiness_removes_without_draining() {
    for readiness in [
        FdReadiness {
            error: true,
            ..FdReadiness::default()
        },
        FdReadiness {
            hangup: true,
            ..FdReadiness::default()
        },
    ] {
        let mut drained = false;
        let action = settings_wake_step(readiness, || {
            drained = true;
            Ok(())
        });
        assert_eq!(action, FdAction::Remove);
        assert!(!drained);
    }
}
