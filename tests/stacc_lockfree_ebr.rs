use std::thread;
use stacc::stacc_lockfree_ebr::*;

#[test]
fn ebr_single() {
    let mut s = Local::new();

    for i in 0..4 {
        s.push(i);
    }

    for i in (0..4).rev() {
        assert_eq!(s.pop(), Some(i));
    }

    assert_eq!(s.pop(), None);
}

#[test]
fn ebr_consumer_producer() {
    let v = Local::new();

    let mut vc = v.clone();
    let sender = thread::spawn(move || {
        for _ in 0..10_000_000 {
            vc.push(1);
        }
    });

    let mut vc = v.clone();
    let reciever = thread::spawn(move || {
        let mut misses = 0;
        for _ in 0..5_000_000 {
            let x = loop {
                match vc.pop() {
                    None => misses += 1,
                    Some(x) => break x,
                }
            };

            assert_eq!(1, x);
        }

        eprintln!("Misses: {}", misses);
    });


    let mut vc = v.clone();
    let reciever2 = thread::spawn(move || {
        let mut misses = 0;
        for _ in 0..5_000_000 {
            let x = loop {
                match vc.pop() {
                    None => misses += 1,
                    Some(x) => break x,
                }
            };

            assert_eq!(1, x);
        }

        eprintln!("Misses: {}", misses);
    });

    sender.join().unwrap();
    reciever.join().unwrap();
    reciever2.join().unwrap();
}
