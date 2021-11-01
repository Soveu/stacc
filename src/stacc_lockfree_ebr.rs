use std::sync::atomic::{fence, AtomicBool, AtomicUsize, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};
use std::mem::MaybeUninit;
use std::ptr;

const MAX_THREADS: usize = 32;

pub struct Node<T> {
    data: MaybeUninit<T>,
    next: *const Node<T>,
}

/* Well, if you happen to own a Node, it means it is outside of stack.
 * That means you can do whatever you want with it */
unsafe impl<T: Send> Send for Node<T> {}

impl<T> Node<T> {
    pub fn uninit() -> Self {
        Self {
            data: MaybeUninit::uninit(),
            next: 0 as *const Self,
        }
    }
}

#[repr(align(64))]
pub struct ThreadLocal {
    current_epoch: AtomicUsize,
    is_active: AtomicBool,
}

impl ThreadLocal {
    const fn new() -> Self {
        Self {
            current_epoch: AtomicUsize::new(0),
            is_active: AtomicBool::new(false),
        }
    }
}

pub struct Shared<T> {
    top: AtomicPtr<Node<T>>,
    threads: [ThreadLocal; MAX_THREADS],
    global_epoch: AtomicUsize,

    /* Unique id for each thread */
    thread_counter: AtomicUsize,
    /* TODO: When `Local` drops, but has still some things in limbo list, it goes here */
    //global_garbage: Mutex<[Vec<*const T>; 3]>,
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        let mut top = *self.top.get_mut();
        while !top.is_null() {
            /* SAFETY: the pointer is non-null, so it must come from Box::into_raw */
            let mut boxed = unsafe { Box::from_raw(top) };
            /* SAFETY: boxed.data must be initialized, because its on stack */
            unsafe { ptr::drop_in_place(boxed.data.as_mut_ptr()); }

            let next = boxed.next;
            drop(boxed);
            top = next as *mut _;
        }
    }
}

impl<T> Shared<T> {
    const fn new() -> Self {
        const THREAD_LOCAL: ThreadLocal = ThreadLocal::new();
        Self {
            top: AtomicPtr::new(ptr::null_mut()),
            threads: [THREAD_LOCAL; MAX_THREADS],
            global_epoch: AtomicUsize::new(0),
            thread_counter: AtomicUsize::new(0),
        }
    }

    /// Returns the previous observed epoch and the new one
    fn start_shared_section(&self, thread_id: usize) -> (usize, usize) {
        self.threads[thread_id].is_active.store(true, Ordering::SeqCst);

        fence(Ordering::Acquire); // It's just nicer to have fresher data

        let current_epoch = self.global_epoch.load(Ordering::Relaxed);
        let old_epoch = self.threads[thread_id].current_epoch.swap(current_epoch, Ordering::Relaxed);
        let have_all_threads_seen_epoch = self.threads
            .iter()
            .filter(|thread| thread.is_active.load(Ordering::Relaxed))
            .map(|thread| thread.current_epoch.load(Ordering::Relaxed))
            .all(|epoch| epoch == current_epoch);

        if have_all_threads_seen_epoch {
            return (old_epoch, current_epoch);
        }

        let next_epoch = match current_epoch.checked_add(1) {
            Some(x) => x,
            None => todo!(),
        };

        /* TODO: maybe if succeeded, clean global garbage */
        /* Many threads can try to increment at the same time, so it is
         * important to use compare_exchange in this place */
        let _has_won_race = self.global_epoch.compare_exchange(
            current_epoch,
            next_epoch,
            Ordering::Release,
            Ordering::Relaxed
        ).is_ok();

        return (old_epoch, current_epoch);
    }

    fn end_shared_section(&self, thread_id: usize) {
        self.threads[thread_id].is_active.store(false, Ordering::Release);
    }
}

pub struct Local<T> {
    shared: Arc<Shared<T>>,
    thread_id: usize,

    limbo: [Vec<*const Node<T>>; 3],
    garbage: Vec<Box<Node<T>>>,
}

impl<T> Local<T> {
    pub fn new() -> Self {
        let shared = Arc::new(Shared::new());
        Self {
            shared,
            thread_id: 0,
            limbo: [Vec::new(), Vec::new(), Vec::new()],
            garbage: Vec::new(),
        }
    }

    /// Safety: `mark_use` must come in pair with `defer`
    fn mark_use(&mut self) {
        let (prev, next) = self.shared.start_shared_section(self.thread_id);
        let diff = std::cmp::min(next - prev, self.limbo.len());

        let iter = self.limbo[..diff]
            .iter_mut()
            .flat_map(|limbo| limbo.drain(..))
            .map(|ptr| unsafe { Box::from_raw(ptr as *mut _) });
        self.garbage.extend(iter);
        self.limbo.rotate_left(diff);
    }

    /// Safety: you can't defer the same pointer more than once.
    /// Must come after `mark_use`
    unsafe fn defer(&mut self, ptr: *const Node<T>) {
        self.shared.end_shared_section(self.thread_id);
        let [.., last] = &mut self.limbo;
        last.push(ptr);
    }

    fn get_node(&mut self, node: Node<T>) -> Box<Node<T>> {
        let mut p = match self.garbage.pop() {
            None => return Box::new(node),
            Some(p) => p,
        };

        *p = node;
        return p;
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
                .compare_exchange_weak(top, node, Ordering::Acquire, Ordering::Acquire)
        {
            /* SAFETY: This pointer must be valid, because it comes from Box::into_raw above */
            unsafe {
                (*node).next = newtop;
            }
            top = newtop;
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        self.mark_use();
        let mut top = self.shared.top.load(Ordering::Acquire);

        let oldtop = loop {
            if top.is_null() {
                return None;
            }

            /* SAFETY: because of EBR, `top` should still be valid */
            let next = unsafe { (*top).next };

            let cas = self.shared.top.compare_exchange_weak(
                top,
                next as *mut _,
                Ordering::Acquire,
                Ordering::Acquire,
            );

            match cas {
                Ok(_) => break top,
                Err(newertop) => top = newertop,
            }
        };

        /* SAFETY: only one thread can succeed at CAS, so we are the only
         * ones reading oldtop.data */
        let data = unsafe { ptr::read((*oldtop).data.as_ptr()) };

        unsafe { self.defer(oldtop); }
        return Some(data);
    }
}

unsafe impl<T: Send> Send for Local<T> {}

impl<T> Clone for Local<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            thread_id: self.shared.thread_counter.fetch_add(1, Ordering::Relaxed),
            limbo: [Vec::new(), Vec::new(), Vec::new()],
            garbage: Vec::new(),
        }
    }
}

impl<T> Drop for Local<T> {
    fn drop(&mut self) {
        self.mark_use();
        /* TODO: don't leak pointers in limbo */
        self.shared.end_shared_section(self.thread_id);
    }
}

