//! A change waits for the next block, the latest write winning, and a block
//! with no room for every change leaves the rest for the one after.

use super::Pending;

#[test]
fn a_change_reaches_the_next_block_once_by_its_host_id() {
    let pending = Pending::new(vec![10, 20, 30]);
    pending.set(1, 0.25);
    let mut changes = [(0, 0.0); 8];
    assert_eq!(pending.take(&mut changes), 1);
    assert_eq!(changes[0], (20, 0.25));
    assert_eq!(pending.take(&mut changes), 0, "taken once");
}

#[test]
fn the_latest_write_wins_and_values_are_clamped() {
    let pending = Pending::new(vec![1, 2]);
    pending.set(0, 0.1);
    pending.set(0, 0.9);
    pending.set(1, 4.0);
    let mut changes = [(0, 0.0); 2];
    assert_eq!(pending.take(&mut changes), 2);
    assert_eq!(changes, [(1, 0.9), (2, 1.0)]);
    pending.set(1, f64::NAN);
    assert_eq!(pending.take(&mut changes), 1);
    assert_eq!(changes[0], (2, 0.0), "a NaN is no change of its own");
}

#[test]
fn changes_past_the_room_wait_for_the_next_block() {
    let pending = Pending::new(vec![1, 2, 3]);
    for index in 0..3 {
        pending.set(index, 0.5);
    }
    let mut room = [(0, 0.0); 2];
    assert_eq!(pending.take(&mut room), 2);
    assert_eq!(room, [(1, 0.5), (2, 0.5)]);
    assert_eq!(pending.take(&mut room), 1);
    assert_eq!(room[0], (3, 0.5));
}

#[test]
fn an_index_past_the_list_is_passed_over() {
    let pending = Pending::new(vec![1]);
    pending.set(5, 0.5);
    assert_eq!(pending.take(&mut [(0, 0.0); 4]), 0);
}
