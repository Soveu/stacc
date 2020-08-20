/* The code tries to be 1:1 copy of LIFO stack from
 * https://cs.nyu.edu/courses/fall16/CSCI-GA.3033-017/readings/hazard_pointers.pdf
 */

use std::mem::MaybeUninit;
use std::ptr;
use std::sync::{atomic::*, Arc};

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
        let mut v: Vec<*const Node<T>> = self
            .shared
            .hazard_pointers
            .iter()
            .map(|x| x.load(Ordering::Acquire) as *const Node<T>)
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
        let mut top = self.shared.top.load(Ordering::Acquire);
        let node = Box::new(Node {
            next: top as *const _,
            data: MaybeUninit::new(data),
        });

        let node = Box::into_raw(node);

        while let Err(newtop) =
            self.shared
                .top
                .compare_exchange_weak(top, node, Ordering::AcqRel, Ordering::Acquire)
        {
            /* SAFETY: This pointer must be valid, because it comes from Box::into_raw above */
            unsafe {
                (*node).next = newtop;
            }
            top = newtop;
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        let mut top = self.shared.top.load(Ordering::Acquire);

        let oldtop = loop {
            self.shared.hazard_pointers[self.thread_number].store(top, Ordering::Release);
            if top.is_null() {
                return None;
            }

            let newertop = self.shared.top.load(Ordering::Acquire);
            if newertop != top {
                top = newertop;
                continue;
            }

            fence(Ordering::Acquire);
            /* SAFETY: We marked the pointer as hazard, so nobody should even try to dealloc it */
            /* UNSAFETY?: could this line be executed before marking it as hazard?
             * Theoretically we have right before a fence, but is it sufficient? */
            let next = unsafe { (*top).next };

            /* Note: maybe change it to compare_exchange_weak? */
            let cas = self.shared.top.compare_exchange(
                top,
                next as *mut _,
                Ordering::AcqRel,
                Ordering::Acquire,
            );

            match cas {
                Ok(oldtop) => break oldtop,
                Err(newertop) => top = newertop,
            }
        };

        /* Ordering is relaxed, because this thread now is responsible for the allocated memory */
        self.shared.hazard_pointers[self.thread_number].store(ptr::null_mut(), Ordering::Relaxed);

        /* SAFETY: only one thread can succeed at CAS, so we are the only
         * ones reading oldtop.data */
        let data = unsafe { ptr::read((*oldtop).data.as_ptr()) };

        self.retire_node(oldtop);
        return Some(data);
    }
}
