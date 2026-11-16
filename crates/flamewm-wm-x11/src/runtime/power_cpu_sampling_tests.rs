use super::*;

#[test]
fn closed_menu_never_arms_even_with_a_stale_deadline() {
    let t0 = Instant::now();
    let stale = Some(t0 + POWER_CPU_SAMPLE_INTERVAL);
    assert_eq!(advance_power_cpu_sampling(false, stale, t0), (false, None));
}

#[test]
fn due_sample_while_open_calls_sampler_and_rearms() {
    let t0 = Instant::now();
    let mut calls = 0;
    let out = tick_power_cpu(true, Some(t0), t0, || {
        calls += 1;
        true
    });
    assert_eq!(calls, 1);
    assert_eq!(out, (true, Some(t0 + POWER_CPU_SAMPLE_INTERVAL)));
}

#[test]
fn closed_menu_never_calls_sampler_and_never_arms() {
    let t0 = Instant::now();
    let mut calls = 0;
    let out = tick_power_cpu(false, Some(t0), t0, || {
        calls += 1;
        true
    });
    assert_eq!(calls, 0);
    assert_eq!(out, (false, None));
}

#[test]
fn not_yet_due_open_menu_does_not_sample() {
    let t0 = Instant::now();
    let due = t0 + POWER_CPU_SAMPLE_INTERVAL;
    let out = tick_power_cpu(true, Some(due), t0, || panic!("sampled before due"));
    assert_eq!(out, (false, Some(due)));
}

#[test]
fn first_open_samples_now_and_arms_one_second() {
    let t0 = Instant::now();
    assert_eq!(
        advance_power_cpu_sampling(true, None, t0),
        (true, Some(t0 + POWER_CPU_SAMPLE_INTERVAL))
    );
}

#[test]
fn open_menu_waits_until_due_then_rearms() {
    let t0 = Instant::now();
    let due = t0 + POWER_CPU_SAMPLE_INTERVAL;
    let half = t0 + POWER_CPU_SAMPLE_INTERVAL / 2;
    assert_eq!(
        advance_power_cpu_sampling(true, Some(due), half),
        (false, Some(due))
    );
    assert_eq!(
        advance_power_cpu_sampling(true, Some(due), due),
        (true, Some(due + POWER_CPU_SAMPLE_INTERVAL))
    );
}
