use std::ptr::{self, NonNull};
use std::mem::MaybeUninit;
use std::sync::{
    Arc,
    Mutex,
    atomic::*,
};

/* NonNull must come from Box::into_raw */
unsafe fn nonnull_to_box<T>(ptr: NonNull<StaccNode<T>>) -> Box<StaccNode<T>> {
    return Box::from_raw(ptr.as_ptr());
}

struct StaccNode<T> {
    next: Option<NonNull<StaccNode<T>>>,
    item: MaybeUninit<T>,
}

impl<T> StaccNode<T> {
    fn new(item: T) -> Self {
        Self {
            next: None,
            item: MaybeUninit::new(item),
        }
    }
}

struct StaccInner<T> {
    head: AtomicPtr<StaccNode<T>>,
    len: AtomicUsize,
    global_garbage: Mutex<Vec<NonNull<StaccNode<T>>>>,
}

impl<T> StaccInner<T> {
    fn new() -> Self {
        Self {
            head: AtomicPtr::new(0 as *mut _),
            len: AtomicUsize::new(0),
            global_garbage: Mutex::new(vec![]),
        }
    }
}

impl<T> Drop for StaccInner<T> {
    fn drop(&mut self) {
        let garbage = self.global_garbage.get_mut().unwrap();

        /* SAFETY: We should be the only one having access to allocated memory */
        garbage.iter()
            .copied()
            .map(|p| unsafe { nonnull_to_box(p) })
            .for_each(drop);

        /* SAFETY: We should be the only one having access to allocated memory */
        while let Some(ptr) = self.pop() {
            let mut boxed = unsafe { nonnull_to_box(ptr) };
            unsafe { ptr::drop_in_place(boxed.item.as_mut_ptr()) };
            drop(boxed);
        }
    }
}

impl<T> StaccInner<T> {
    fn pop(&self) -> Option<NonNull<StaccNode<T>>> {
        loop {
            let head = self.head.load(Ordering::Acquire);
            let head = NonNull::new(head)?;

            /* SAFETY: head is non-null, so it should be pointing to right element */
            let headref = unsafe { head.as_ref() };

            let newhead = match headref.next {
                None => 0 as *mut _,
                Some(p) => p.as_ptr(),
            };

            let x = self.head.compare_exchange_weak(
                head.as_ptr(),
                newhead,
                Ordering::Acquire,
                Ordering::Relaxed,
            );

            if x.is_ok() { 
                self.len.fetch_sub(1, Ordering::Relaxed);
                return Some(head);
            }
        };
    }

    fn push(&self, mut node: Box<StaccNode<T>>) {
        let head = self.head.load(Ordering::Acquire);
        node.next = NonNull::new(head);
        let node = Box::into_raw(node);

        while let Err(newhead) = self.head.compare_exchange(
            head,
            node,
            Ordering::Acquire,
            Ordering::Relaxed)
        {
            /* SAFETY: we own the allocated object, so it must still exist */
            unsafe { (*node).next = NonNull::new(newhead) };
        }

        self.len.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct Stacc<T> {
    inner: Arc<StaccInner<T>>,
    local_garbage: Vec<NonNull<StaccNode<T>>>,
}

impl<T> Stacc<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(StaccInner::new()),
            local_garbage: vec![],
        }
    }

    fn make_node(&mut self, x: T) -> Box<StaccNode<T>> {
        if let Some(nonnull) = self.local_garbage.pop() {
            /* SAFETY: local_garbage should have only pointers that come from Box::into_raw */
            unsafe {
                let mut boxed = Box::from_raw(nonnull.as_ptr());
                boxed.item = MaybeUninit::new(x);
                return boxed;
            }
        }

        let mut global_garbage = self.inner.global_garbage.lock().unwrap();
        std::mem::swap(&mut *global_garbage, &mut self.local_garbage);
        drop(global_garbage);

        if let Some(nonnull) = self.local_garbage.pop() {
            /* SAFETY: local_garbage should have only pointers that come from Box::into_raw */
            unsafe {
                let mut boxed = Box::from_raw(nonnull.as_ptr());
                boxed.item = MaybeUninit::new(x);
                return boxed;
            }
        }

        return Box::new(StaccNode::new(x));
    }
    pub fn pop(&mut self) -> Option<T> {
        let nonnull = self.inner.pop()?;

        /* SAFETY: `pop()?` should give us only valid pointers */
        let item = unsafe {
            ptr::read(&nonnull.as_ref().item).assume_init()
        };

        self.local_garbage.push(nonnull);
        return Some(item);
    }

    pub fn push(&mut self, x: T) {
        let node = self.make_node(x);
        self.inner.push(node);
    }

    pub fn len(&self) -> usize {
        self.inner.len.load(Ordering::Relaxed)
    }
}

impl<T> Clone for Stacc<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            local_garbage: Vec::new(),
        }
    }
}

impl<T> Drop for Stacc<T> {
    fn drop(&mut self) {
        let mut global_garbage = self.inner.global_garbage.lock().unwrap();
        global_garbage.append(&mut self.local_garbage);
    }
}

