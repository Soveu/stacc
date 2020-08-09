use std::thread;
use stacc::*;

#[test]
fn single() {
    let v = Stacc::new(4);

    for i in 0..4 {
        assert_eq!(v.push(i), None);
    }
    for i in (0..4).rev() {
        let x = v.pop();
        assert_eq!(x, Some(i));
    }
}

#[test]
fn multi() {
    let v = Stacc::new(4096);

    let mut threads = Vec::with_capacity(4);
    for i in 0..4 {
        let vc = v.clone();
        threads.push(thread::spawn(move || {
            let from = i * 1024;
            let to = from + 1024;
            for j in from..to {
                assert_eq!(vc.push(j), None);
            }
        }));
    }

    for t in threads {
        t.join().unwrap();
    }

    let mut sum = 4096 * (4096 - 1) / 2;
    for _ in 0..4096 {
        sum -= v.pop().unwrap();
    }
    assert_eq!(sum, 0);
}

#[test]
fn multi2() {
    let v = Stacc::new(2);

    let mut threads = Vec::with_capacity(4);
    for _ in 0..4 {
        let vc = v.clone();
        threads.push(thread::spawn(move || {
            let mut push_misses = 0;
            let mut pop_misses = 0;
            for _ in 0..1_000_000 {
                while vc.push(1).is_some() { push_misses += 1 }
                while vc.pop().is_none() { pop_misses += 1 }
                while vc.push(1).is_some() { push_misses += 1 }
                while vc.pop().is_none() { pop_misses += 1 }
                while vc.push(1).is_some() { push_misses += 1 }
                while vc.pop().is_none() { pop_misses += 1 }
            }

            eprintln!("Pop misses: {}, push misses: {}", pop_misses, push_misses);
        }));
    }

    for t in threads {
        t.join().unwrap();
    }

    eprintln!("{}", v.len());
    assert_eq!(v.pop(), None);
}

#[test]
fn multi3() {
    let v = Stacc::new(4096);

    for _ in 0..1024 {
        v.push(1);
    }

    let mut threads = Vec::with_capacity(4);
    for _ in 0..4 {
        let vc = v.clone();
        threads.push(thread::spawn(move || {
            let mut push_misses = 0;
            let mut pop_misses = 0;
            for _ in 0..1_000_000 {
                while vc.push(1).is_some() { push_misses += 1 }
                while vc.push(1).is_some() { push_misses += 1 }
                while vc.pop().is_none() { pop_misses += 1 }
                while vc.push(1).is_some() { push_misses += 1 }
                while vc.pop().is_none() { pop_misses += 1 }
                while vc.pop().is_none() { pop_misses += 1 }
            }

            eprintln!("Pop misses: {}, push misses: {}", pop_misses, push_misses);
        }));
    }

    for t in threads {
        t.join().unwrap();
    }

    eprintln!("{}", v.len());
    assert_eq!(v.len(), 1024);
}

#[test]
fn multi4() {
    let v = Stacc::new(4096);

    let vc = v.clone();
    let sender = thread::spawn(move || {
        let mut push_misses = 0;
        for _ in 0..10_000 {
            while vc.push(1).is_some() { push_misses += 1 }
        }
        eprintln!("Push misses: {}", push_misses);
    });

    let vc = v.clone();
    let reciever = thread::spawn(move || {
        let mut pop_misses = 0;
        for _ in 0..10_000 {
            while vc.pop().is_none() { pop_misses += 1 }
        }
        eprintln!("Pop misses: {}", pop_misses);
    });

    sender.join().unwrap();
    reciever.join().unwrap();

    eprintln!("{}", v.len());
}

