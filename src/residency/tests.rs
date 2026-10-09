use super::Residency;

#[cfg(web)]
use wasm_bindgen_test::wasm_bindgen_test as test;

#[test]
fn models_are_due_after_the_idle_minutes_with_the_call_stopped() {
    let mut residency = Residency::new(10);
    residency.observe(1_000, false);
    assert_eq!(residency.deadline(), None, "in use");
    residency.observe(5_000, true);
    residency.observe(9_000, true);
    assert_eq!(
        residency.deadline(),
        Some(5_000 + 600_000),
        "idle from the first time it was"
    );
    assert!(!residency.due(604_999));
    assert!(residency.due(605_000));
    assert!(!residency.due(700_000), "once");
    assert_eq!(residency.deadline(), None);
}

#[test]
fn using_them_again_stops_the_clock_and_zero_minutes_is_at_once() {
    let mut residency = Residency::new(10);
    residency.observe(0, true);
    residency.observe(1_000, false);
    assert!(!residency.due(1_000_000));
    residency.set_minutes(0);
    residency.observe(2_000, true);
    assert!(residency.due(2_000));
}
