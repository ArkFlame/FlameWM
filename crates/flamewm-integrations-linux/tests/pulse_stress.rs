#![cfg(feature = "pulse-audio")]

//! Wave 3 audio stress: 200 typed ops through the real `PulseAdapter`.
//! Restores original default-sink volume+mute in a finally path even on failure.

use std::time::{Duration, Instant};

use flamewm_api::system::{
    AudioEndpointKind, AudioMuteAction, AudioSnapshot, AudioTarget, AudioVolumeAction,
    ServiceAvailability,
};
use flamewm_integrations_linux::pulse::PulseAdapter;

fn wait_ready(adapter: &PulseAdapter) -> Option<AudioSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let snap = adapter.snapshot();
        if snap.availability == ServiceAvailability::Available && !snap.sink_name.is_empty() {
            return Some(snap);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

fn restore(adapter: &PulseAdapter, target: AudioTarget, volume: u8, muted: bool) {
    let snap = adapter.snapshot();
    let _ = adapter.set_volume(AudioVolumeAction {
        target,
        percent: volume,
        generation: snap.generation,
        server_generation: snap.server_generation,
    });
    std::thread::sleep(Duration::from_millis(200));
    let snap = adapter.snapshot();
    let _ = adapter.set_mute(AudioMuteAction {
        target,
        muted,
        generation: snap.generation,
        server_generation: snap.server_generation,
    });
    std::thread::sleep(Duration::from_millis(200));
}

#[test]
fn pulse_provider_stress() {
    const TOTAL_OPS: usize = 200;
    let adapter = PulseAdapter::connect(|_| {}).expect("PulseAdapter::connect must succeed");

    let first = wait_ready(&adapter)
        .expect("provider must become Available (PipeWire-Pulse live) within 15s");
    let orig_volume = first.volume_percent;
    let orig_muted = first.muted;
    let orig_target = first
        .endpoints()
        .iter()
        .find(|e| e.kind == AudioEndpointKind::Sink && e.name == first.sink_name)
        .map(|e| AudioTarget::Endpoint {
            id: e.id,
            kind: AudioEndpointKind::Sink,
        })
        .expect("default sink endpoint must exist");

    // Seeded LCG pseudo-random percents in 5..=100.
    let mut seed: u64 = 0x1234_5678_9abc_def1;
    let mut next_rand = move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (5 + (seed >> 33) % 96) as u8
    };

    let mut completed: usize = 0;
    let mut failure: Option<String> = None;
    let mut last_target = orig_target;

    for i in 0..TOTAL_OPS {
        let snap = adapter.snapshot();
        if snap.availability != ServiceAvailability::Available {
            failure = Some(format!("provider unavailable at op {i}/{TOTAL_OPS}"));
            break;
        }
        let target_opt = snap
            .endpoints()
            .iter()
            .find(|e| e.kind == AudioEndpointKind::Sink && e.name == snap.sink_name)
            .map(|e| AudioTarget::Endpoint {
                id: e.id,
                kind: AudioEndpointKind::Sink,
            });
        let Some(target) = target_opt else {
            failure = Some(format!("default sink vanished at op {i}/{TOTAL_OPS}"));
            break;
        };
        last_target = target;

        if i % 2 == 0 {
            let percent = if i % 4 == 0 {
                5 + ((i / 4 * 5) % 96) as u8
            } else {
                next_rand()
            };
            if let Err(e) = adapter.set_volume(AudioVolumeAction {
                target,
                percent,
                generation: snap.generation,
                server_generation: snap.server_generation,
            }) {
                failure = Some(format!("set_volume({percent}) failed at op {i}: {e:?}"));
                break;
            }
        } else {
            let muted = (i / 2) % 2 == 0;
            if let Err(e) = adapter.set_mute(AudioMuteAction {
                target,
                muted,
                generation: snap.generation,
                server_generation: snap.server_generation,
            }) {
                failure = Some(format!("set_mute({muted}) failed at op {i}: {e:?}"));
                break;
            }
        }
        completed += 1;
        if i % 25 == 24 {
            let s = adapter.snapshot();
            if s.availability != ServiceAvailability::Available {
                failure = Some(format!("provider died after batch ending at op {i}"));
                break;
            }
            let _ = adapter.generation();
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    // Finally path: always restore original volume+mute, even on failure.
    restore(&adapter, last_target, orig_volume, orig_muted);
    std::thread::sleep(Duration::from_millis(400));
    let after = adapter.snapshot();
    let restored_ok = after.volume_percent == orig_volume && after.muted == orig_muted;

    println!("PULSE_STRESS completed_ops={completed}/{TOTAL_OPS}");
    println!("PULSE_STRESS orig_volume={orig_volume} orig_muted={orig_muted}");
    println!(
        "PULSE_STRESS final_volume={} final_muted={} restore_verified={restored_ok}",
        after.volume_percent, after.muted
    );
    println!(
        "PULSE_STRESS STATUS={}",
        if failure.is_none() && completed == TOTAL_OPS && restored_ok {
            "PASS"
        } else {
            "FAIL"
        }
    );
    assert!(
        failure.is_none(),
        "stress failed: {}",
        failure.unwrap_or_default()
    );
    assert_eq!(completed, TOTAL_OPS, "all ops must complete");
    assert!(
        restored_ok,
        "original volume/mute must be restored (orig {orig_volume}/{orig_muted}, got {}/{})",
        after.volume_percent, after.muted
    );
}
