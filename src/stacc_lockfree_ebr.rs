use std::sync::atomic::{fence, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const MAX_THREADS: usize = 32;

pub struct Shared<T> {
    /* Because we don't need whole 8 bits, the first one is 
     * used as a 'thread active' bit */
    thread_epochs: [AtomicU8; MAX_THREADS],
    global_epoch: AtomicU8,

    /* Unique id for each thread */
    thread_counter: AtomicUsize,
    /* When `Local` drops, but has still some things in limbo list, it goes here */
    global_garbage: Mutex<[Vec<*const T>; 4]>,
}

impl<T> Shared<T> {
    /// Returns the newest epoch and whether all threads have observed it
    fn start_shared_section(&self, thread_id: usize) -> (u8, bool) {
        let current_counter: u8 = self.global_epoch.load(Ordering::SeqCst);
        let epoch = (current_counter << 1) | 1;
        self.thread_epochs[thread_id].store(epoch, Ordering::SeqCst);

        fence(Ordering::Acquire); // It's just nicer to have fresher data

        for epoch in self.thread_epochs
            .iter()
            .map(|x| x.load(Ordering::Relaxed))
            .filter(|epoch| (epoch & 1) != 0)
        {
            if (epoch >> 1) != current_counter {
                return (current_counter, false);
            }
        }

        /* bitwise AND this, just in case */
        let next_counter = current_counter.wrapping_add(1) & 0b0000_0011;

        /* TODO: maybe if succeeded, clean global garbage */
        /* Many threads can try to increment at the same time, so it is
         * important to use compare_exchange in this place */
        let _ = self.global_epoch.compare_exchange(
            current_counter,
            next_counter,
            Ordering::SeqCst,
            Ordering::SeqCst
        );

        return (current_counter, true);
    }

    /// Returns the epoch that thread had while starting shared section
    fn end_shared_section(&self, thread_id: usize) -> u8 {
        let epoch = self.thread_epochs[thread_id].load(Ordering::Relaxed);
        self.thread_epochs[thread_id].store(epoch & 0b0000_0110, Ordering::Release);
        return epoch >> 1;
    }
}

pub struct Local<T> {
    shared: Arc<Shared<T>>,
    limbo: [Vec<*const T>; 4],

    thread_id: usize,
    garbage: Vec<Box<T>>,
}

impl<T> Local<T> {
    /// Safety: `mark_use` must come in pair with `defer`
    pub unsafe fn mark_use(&mut self) {
        let (current_counter, have_threads_observed_epoch) = 
            self.shared.start_shared_section(self.thread_id);

        if !have_threads_observed_epoch {
            return;
        }

        let two_ago = current_counter.wrapping_sub(2) % 4;
        let two_ago = two_ago as usize;

        /* Safety: since nobody uses these pointers, we can wrap them back into Box */
        let iter = self.limbo[two_ago].drain(..)
            .map(|p| unsafe { Box::from_raw(p as *mut T) });

        self.garbage.extend(iter);
    }

    /// Safety: you can't defer the same pointer more than once.
    /// Must come after `mark_use`
    pub unsafe fn defer(&mut self, ptr: *const T) {
        let epoch = self.shared.end_shared_section(self.thread_id);
        self.limbo[epoch as usize].push(ptr);
    }
}

