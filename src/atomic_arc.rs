use std::sync::{
    Arc,
    atomic::*,
};

pub struct AtomicArc<T> {
    ptr: AtomicPtr<T>,
}

unsafe fn arc_from_ptr<T>(ptr: *const T) -> Option<Arc<T>> {
    if ptr.is_null() {
        return None;
    }
    Some(Arc::from_raw(ptr))
}

fn ptr_from_arc<T>(arc: Option<Arc<T>>) -> *const T {
    match arc {
        None => 0 as _,
        Some(a) => Arc::into_raw(a),
    }
}

impl<T> AtomicArc<T> {
    pub fn from_arc(arc: Arc<T>) -> Self {
        let ptr = Arc::into_raw(arc) as *mut T;
        let ptr: AtomicPtr<T> = ptr.into();
        Self { ptr }
    }

    pub fn load(&self) -> Option<Arc<T>> {
        let ptr = self.ptr.load(Ordering::Acquire);
        if ptr.is_null() {
            return None;
        }

        let arc = unsafe {
            let oldarc = Arc::from_raw(ptr);
            let newarc = oldarc.clone();
            std::mem::forget(oldarc);
            newarc
        };

        return Some(arc);
    }

    pub fn swap(&self, other: Option<Arc<T>>) -> Option<Arc<T>> {
        let ptr = ptr_from_arc(other);
        let ptr = self.ptr.swap(ptr as *mut T, Ordering::AcqRel);
        unsafe { arc_from_ptr(ptr) }
    }

    pub fn compare_maybe_exchange(&self,
        current: *const T,
        new: Option<Arc<T>>,
    ) -> Result<Option<Arc<T>>, Option<Arc<T>>> {
        let new_ptr = ptr_from_arc(new);

        let x = self.ptr.compare_exchange_weak(
            current as *mut T,
            new_ptr as *mut T,
            Ordering::AcqRel,
            Ordering::Relaxed
        );

        return match x {
            Ok(p) => unsafe { Ok(arc_from_ptr(p)) },
            Err(_) => unsafe { Err(arc_from_ptr(new_ptr)) }, 
        };
    }
}

impl<T> Drop for AtomicArc<T> {
    fn drop(&mut self) {
        self.swap(None);
    }
}

