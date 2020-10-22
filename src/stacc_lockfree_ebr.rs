use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const MAX_THREADS: usize = 32;

pub struct Shared<T> {
    thread_epochs: [AtomicU8; MAX_THREADS],
    global_epoch: AtomicU8,

    /* Unique id for each thread */
    thread_counter: AtomicUsize,
    /* When `Local` drops, but has still some things in limbo list, it goes here */
    global_garbage: Mutex<[Vec<*const T>; 4]>,
}

impl<T> Shared<T> {
    fn have_all_observed_epoch(&self, value: u8) -> bool {
        for epoch in self.thread_epochs.iter().map(|x| x.load(Ordering::SeqCst)) {
            if (epoch & 1) == 0 {
                continue;
            }
            if (epoch >> 1) != value {
                return false;
            }
        }

        self.global_epoch.fetch_add(1, Ordering::SeqCst);
        return true;
    }
}

pub struct Local<T> {
    shared: Arc<Shared<T>>,
    limbo: [Vec<*const T>; 4],

    thread_id: usize,
    garbage: Vec<Box<T>>,
}

impl<T> Local<T> {
    /* Safety: `mark_use` must come in pair with `defer` */
    pub unsafe fn mark_use(&mut self) {
        let current_counter: u8 = self.shared.global_epoch.load(Ordering::SeqCst);
        let epoch = (current_counter << 1) | 1;
        self.shared.thread_epochs[self.thread_id].store(epoch, Ordering::SeqCst);

        if self.shared.have_all_observed_epoch(current_counter) {
            let two_ago = current_counter.wrapping_sub(2) % 4;
            let two_ago = two_ago as usize;

            /* Safety: since nobody uses these pointers, we can wrap them back into Box */
            let iter = self.limbo[two_ago].iter()
                .copied()
                .map(|p| unsafe { Box::from_raw(p as *mut T) });

            self.garbage.extend(iter);
            self.limbo[two_ago].clear();
        }
    }

    /* Safety: you can't defer the same pointer more than once,
     * must come after `mark_use` */
    pub unsafe fn defer(&mut self, ptr: *const T) {
        let epoch = self.shared.thread_epochs[self.thread_id].fetch_sub(1, Ordering::SeqCst);
        let epoch = epoch >> 1;
        self.limbo[epoch as usize].push(ptr);
    }
}

