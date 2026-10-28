// Ports: dataset/dict_test.go @ add21b5

//! Go's `dict_test.go`, case for case.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::FreqDict;

#[test]
fn freq_dict() {
    let mut dict = FreqDict::new();
    assert_eq!(dict.add("a"), 0);
    assert_eq!(dict.add("b"), 1);
    assert_eq!(dict.add("b"), 1);
    assert_eq!(dict.add("c"), 2);
    assert_eq!(dict.add("c"), 2);
    assert_eq!(dict.add("c"), 2);
    assert_eq!(dict.count(), 3);
    assert_eq!(dict.freq(0), 1);
    assert_eq!(dict.freq(1), 2);
    assert_eq!(dict.freq(2), 3);
    assert_eq!(dict.id("a"), 0);
    assert_eq!(dict.id("e"), -1);
}

#[test]
fn freq_dict_lock() {
    let mut dict = FreqDict::new();
    let index = dict.add_no_count("a");
    dict.lock(index);

    let (locked_tx, locked_rx) = mpsc::channel();
    let (released_tx, released_rx) = mpsc::channel();
    let acquired = AtomicBool::new(false);
    let shared = &dict;
    let acquired_ref = &acquired;
    thread::scope(|scope| {
        scope.spawn(move || {
            locked_tx.send(()).expect("the test is waiting");
            shared.lock(index);
            acquired_ref.store(true, Ordering::SeqCst);
            shared.unlock(index);
            released_tx.send(()).expect("the test is waiting");
        });

        locked_rx.recv().expect("the goroutine starts");
        assert!(
            released_rx.recv_timeout(Duration::from_millis(10)).is_err(),
            "lock should block concurrent access"
        );
        assert!(!acquired_ref.load(Ordering::SeqCst));

        shared.unlock(index);

        released_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("lock should be released");
        assert!(acquired_ref.load(Ordering::SeqCst));
    });
}
