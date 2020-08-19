use std::sync::{
    atomic::*,
    Arc,
};
use std::mem::MaybeUninit;
use std::ptr;

const MAX_THREADS: usize = 64;
const R: usize = 8; // chosen by dice roll

struct Node<T> {
    data: MaybeUninit<T>,
    next: *const Node<T>,
}

struct Shared<T> {
    top: AtomicPtr<Node<T>>,
    hazard_pointers: [AtomicPtr<Node<T>>; MAX_THREADS],
}

pub struct Private<T> {
    shared: Arc<Shared<T>>,
    retired_pointers: Vec<*const Node<T>>,
    thread_number: usize,
}

impl<T> Private<T> {
    fn prepare_for_reuse(&mut self, _boxed: Box<Node<T>>) {
        /* currently, just drop */
    }

    fn scan(&mut self) {
        let mut v: Vec<*const Node<T>> = self.shared.hazard_pointers.iter()
            .map(|x| x.load(Ordering::Relaxed) as *const Node<T>)
            .filter(|p| !p.is_null())
            .collect();

        v.sort_unstable();
        let mut rlist = std::mem::replace(&mut self.retired_pointers, Vec::new());
        for ptr in rlist.drain_filter(|x| v.binary_search(x).is_err()) {
            /* SAFETY: pointers from Box::into_raw */
            debug_assert!(!ptr.is_null());
            let boxed = unsafe { Box::from_raw(ptr as *mut Node<T>) };
            self.prepare_for_reuse(boxed);
        }

        self.retired_pointers = rlist;
    }

    fn retire_node(&mut self, node: *const Node<T>) {
        self.retired_pointers.push(node);
        if self.retired_pointers.len() >= R {
            self.scan();
        }
    }

    pub fn push(&mut self, data: T) {
        let node = Box::new(Node {
            next: 0 as *const _,
            data: MaybeUninit::new(data),
        });

        let node = Box::into_raw(node);

        loop {
            let t = self.shared.top.load(Ordering::Acquire);

            /* SAFETY: This pointer must be valid, because it comes from Box::into_raw above */
            unsafe { (*node).next = t; }
            let cas = self.shared.top.compare_exchange_weak(
                t,
                node,
                Ordering::Acquire,
                Ordering::Relaxed,
            );

            if cas.is_ok() {
                return;
            }
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        loop {
            let top = self.shared.top.load(Ordering::Acquire);
            if top.is_null() {
                /* Note(Soveu): should we zero out the hazard pointer? */
                return None;
            }

            self.shared.hazard_pointers[self.thread_number].store(top, Ordering::Release);
            if self.shared.top.load(Ordering::Acquire) != top {
                /* Note(Soveu): should we zero out the hazard pointer? */
                continue;
            }

            /* SAFETY: ??? 
             * The code tries to be 1:1 copy of LIFO stack from 
             * https://cs.nyu.edu/courses/fall16/CSCI-GA.3033-017/readings/hazard_pointers.pdf
             */
            let next = unsafe { (*top).next };
            let cas = self.shared.top.compare_exchange(
                top,
                next as *mut _,
                Ordering::Acquire,
                Ordering::Relaxed,
            );

            if cas.is_ok() {
                /* SAFETY: ??? + see safety above */
                let data = unsafe {
                    ptr::read((*top).data.as_ptr())
                };
                self.retire_node(top);
                return Some(data);
            }
        }
    }
}

