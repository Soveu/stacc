/* The code tries to be 1:1 copy of LIFO stack from
 * https://cs.nyu.edu/courses/fall16/CSCI-GA.3033-017/readings/hazard_pointers.pdf
 */

use std::mem::MaybeUninit;
use std::ptr;
use std::sync::Mutex;
use std::sync::{atomic::*, Arc};

/* 32, because arrays implement Default only up to 32 elements :( */
const MAX_THREADS: usize = 32;
const R: usize = 42;

struct Node<T> {
    data: MaybeUninit<T>,
    next: *const Node<T>,
}

struct Shared<T> {
    top: AtomicPtr<Node<T>>,
    hazard_pointers: [AtomicPtr<Node<T>>; MAX_THREADS],

    /* If a Private is being dropped, but some pointers are still marked as
     * hazard, they end up here */
    boxes_that_are_still_hazard: Mutex<Vec<*const Node<T>>>,
    /* Used to give unique ID for each thread */
    counter: AtomicUsize,

    /* (Optional) Purely for statistics, is updated using relaxed ordering */
    len: AtomicUsize,
}

impl<T> Shared<T> {
    fn new() -> Self {
        Self {
            top: AtomicPtr::new(ptr::null_mut()),
            hazard_pointers: Default::default(),
            boxes_that_are_still_hazard: Mutex::new(Vec::new()),
            counter: AtomicUsize::new(0),
            len: AtomicUsize::new(0),
        }
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        let v: &mut Vec<_> = self.boxes_that_are_still_hazard.get_mut().unwrap();

        for ptr in v.iter().copied() {
            /* SAFETY: pointer is from Box::into_raw and we are the only ones having it */
            debug_assert!(!ptr.is_null());
            let boxed = unsafe { Box::from_raw(ptr as *mut Node<T>) };
            drop(boxed);
        }

        let mut top = *self.top.get_mut();
        while !top.is_null() {
            let next = unsafe { (*top).next };
            let boxed = unsafe { Box::from_raw(top) };
            drop(boxed);
            top = next as *mut _;
        }
    }
}

pub struct Private<T> {
    shared: Arc<Shared<T>>,
    retired_pointers: Vec<*const Node<T>>,
    thread_number: usize,

    /* (Optional) reduces calls to alloc() and dealloc() */
    cached_allocations: Vec<Box<Node<T>>>,
}

unsafe impl<T> Send for Private<T> {}

impl<T> Private<T> {
    pub fn new() -> Self {
        let shared = Shared::new();
        Self {
            thread_number: shared.counter.fetch_add(1, Ordering::Relaxed),
            shared: Arc::new(shared),
            retired_pointers: Vec::new(),
            cached_allocations: Vec::new(),
        }
    }
    pub fn drop_cached_allocations(&mut self) {
        self.cached_allocations.clear()
    }

    fn get_node(&mut self, node: Node<T>) -> Box<Node<T>> {
        match self.cached_allocations.pop() {
            None => Box::new(node),
            Some(b) => b,
        }
    }
    fn prepare_for_reuse(&mut self, boxed: Box<Node<T>>) {
        self.cached_allocations.push(boxed);
    }

    fn scan(&mut self) {
        /* It shouldn't be needed, but its just nice to have fresher data */
        fence(Ordering::Acquire);

        let mut v: Vec<*const Node<T>> = self
            .shared
            .hazard_pointers
            .iter()
            .map(|x| x.load(Ordering::Relaxed) as *const Node<T>)
            .filter(|p| !p.is_null())
            .collect();

        v.sort_unstable();
        let mut rlist = std::mem::replace(&mut self.retired_pointers, Vec::new());

        for ptr in rlist.drain_filter(|x| v.binary_search(x).is_err()) {
            /* SAFETY: pointer is from Box::into_raw and we are the only ones having it */
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
        let node = Node {
            next: top as *const _,
            data: MaybeUninit::new(data),
        };
        let node = self.get_node(node);
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

        self.shared.len.fetch_add(1, Ordering::Relaxed);
    }

    pub fn pop(&mut self) -> Option<T> {
        let mut top = self.shared.top.load(Ordering::Acquire);

        let oldtop = loop {
            /* SeqCst is _very_ important here, thanks Acrimon for pointing it out */
            self.shared.hazard_pointers[self.thread_number].store(top, Ordering::SeqCst);
            if top.is_null() {
                return None;
            }

            let newertop = self.shared.top.load(Ordering::SeqCst);
            if newertop != top {
                top = newertop;
                continue;
            }

            /* SAFETY: We marked the pointer as hazard, so nobody should even try to dealloc it.
             * Compiler is forced to put this after fences.
             * Hardware can pre-fetch the result (because of speculative execution), but it
             * shouldn't change correctness of this code, because top.next is a constant.
             * Also, it shouldn't cause segfault, unlike software instruction reordering. */
            let next = unsafe { (*top).next };

            let cas = self.shared.top.compare_exchange_weak(
                top,
                next as *mut _,
                Ordering::SeqCst,
                Ordering::Acquire,
            );

            match cas {
                Ok(oldtop) => break oldtop,
                Err(newertop) => top = newertop,
            }
        };

        /* Ordering is relaxed, because this thread now is responsible for the allocated memory */
        self.shared.hazard_pointers[self.thread_number].store(ptr::null_mut(), Ordering::Relaxed);
        self.shared.len.fetch_sub(1, Ordering::Relaxed);

        /* SAFETY: only one thread can succeed at CAS, so we are the only
         * ones reading oldtop.data */
        let data = unsafe { ptr::read((*oldtop).data.as_ptr()) };

        self.retire_node(oldtop);
        return Some(data);
    }

    pub fn len(&self) -> usize {
        self.shared.len.load(Ordering::Relaxed)
    }
}

impl<T> Drop for Private<T> {
    fn drop(&mut self) {
        self.shared.hazard_pointers[self.thread_number].store(ptr::null_mut(), Ordering::Release);
        self.scan();
        let mut lock = self.shared.boxes_that_are_still_hazard.lock().unwrap();
        lock.append(&mut self.retired_pointers);
    }
}

impl<T> Clone for Private<T> {
    fn clone(&self) -> Self {
        let shared = Arc::clone(&self.shared);
        let thread_number = shared.counter.fetch_add(1, Ordering::AcqRel);
        Self {
            shared,
            thread_number,
            retired_pointers: Vec::new(),
            cached_allocations: Vec::new(),
        }
    }
}
