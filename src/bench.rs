extern crate crossbeam;
extern crate syncbox;
extern crate test;

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{Ordering, AtomicUsize, AtomicPtr};
use std::mem;

use self::crossbeam::sync::{MsQueue, SegQueue, TreiberStack};
use self::syncbox::ArrayQueue;
use self::test::{Bencher, black_box};

use {CreateFn, Pool, SpinLock};

#[derive(Debug)]
struct Dummy(usize);

fn dummy() -> CreateFn<Box<Dummy>> {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::SeqCst;

    let count = AtomicUsize::new(0);
    Box::new(move || {
        Box::new(Dummy(count.fetch_add(1, SeqCst)))
    })
}

#[bench]
fn spin_lock_unlock(b: &mut Bencher) {
    let lock = SpinLock::new();
    b.iter(|| {
        black_box({
            lock.lock();
            lock.unlock();
        })
    });
}

#[bench]
fn mutex_lock_unlock(b: &mut Bencher) {
    let lock = Mutex::new(());
    b.iter(|| {
        black_box({
            let lock = lock.lock().unwrap();
            drop(lock);
        })
    });
}

#[bench]
fn mempool_get_put_arc(b: &mut Bencher) {
    let pool = Pool::new(dummy());
    let _ = pool.get();
    b.iter(|| {
        black_box({
            let data = pool.get();
            drop(data);
        })
    });
}

#[bench]
fn mempool_get_put_ref(b: &mut Bencher) {
    let pool = Pool::new(dummy());
    let _ = pool.get_ref();
    b.iter(|| {
        black_box({
            let data = pool.get_ref();
            drop(data);
        })
    });
}

#[bench]
fn refcell_get_put(b: &mut Bencher) {
    let pool = PoolRefCell::new(dummy());
    let _ = pool.get();
    b.iter(|| {
        black_box({
            let data = pool.get();
            drop(data);
        })
    });
}

#[bench]
fn mutex_get_put(b: &mut Bencher) {
    let pool = PoolMutex::new(dummy());
    let _ = pool.get();
    b.iter(|| {
        black_box({
            let data = pool.get();
            drop(data);
        })
    });
}

#[bench]
fn mpmc_get_put(b: &mut Bencher) {
    let pool = PoolMpmc::new(dummy());
    let _ = pool.get();
    b.iter(|| {
        black_box({
            let data = pool.get();
            drop(data);
        })
    });
}

#[bench]
fn lossy_mpmc_get_put(b: &mut Bencher) {
    let pool = PoolLossyMpmc::new(dummy());
    let _ = pool.get();
    b.iter(|| {
        black_box({
            let data = pool.get();
            drop(data);
        })
    });
}

#[bench]
fn crossbeam_treiber_get_put(b: &mut Bencher) {
    let pool = PoolTreiber::new(dummy());
    let _ = pool.get();
    b.iter(|| {
        black_box({
            let data = pool.get();
            drop(data);
        })
    });
}

#[bench]
fn crossbeam_ms_get_put(b: &mut Bencher) {
    let pool = PoolMs::new(dummy());
    let _ = pool.get();
    b.iter(|| {
        black_box({
            let data = pool.get();
            drop(data);
        })
    });
}

#[bench]
fn crossbeam_seg_get_put(b: &mut Bencher) {
    let pool = PoolSeg::new(dummy());
    let _ = pool.get();
    b.iter(|| {
        black_box({
            let data = pool.get();
            drop(data);
        })
    });
}

struct PoolRefCell<T> {
    stack: RefCell<Vec<T>>,
    create: CreateFn<T>,
}

struct PoolRefCellGuard<'a, T: 'a> {
    pool: &'a PoolRefCell<T>,
    data: Option<T>,
}

impl<T> PoolRefCell<T> {
    fn new(create: CreateFn<T>) -> PoolRefCell<T> {
        PoolRefCell { stack: RefCell::new(vec![]), create: create }
    }

    fn get(&self) -> PoolRefCellGuard<T> {
        let mut stack = self.stack.borrow_mut();
        match stack.pop() {
            None => {
                PoolRefCellGuard { pool: self, data: Some((self.create)()) }
            }
            Some(data) => PoolRefCellGuard { pool: self, data: Some(data) }
        }
    }

    fn put(&self, data: T) {
        let mut stack = self.stack.borrow_mut();
        stack.push(data);
    }
}

impl<'a, T> Drop for PoolRefCellGuard<'a, T> {
    fn drop(&mut self) {
        let data = self.data.take().unwrap();
        self.pool.put(data);
    }
}

struct PoolMutex<T> {
    stack: Mutex<Vec<T>>,
    create: CreateFn<T>,
}

struct PoolMutexGuard<'a, T: 'a> {
    pool: &'a PoolMutex<T>,
    data: Option<T>,
}

impl<T> PoolMutex<T> {
    fn new(create: CreateFn<T>) -> PoolMutex<T> {
        PoolMutex { stack: Mutex::new(vec![]), create: create }
    }

    fn get(&self) -> PoolMutexGuard<T> {
        let mut stack = self.stack.lock().unwrap();
        match stack.pop() {
            None => {
                PoolMutexGuard { pool: self, data: Some((self.create)()) }
            }
            Some(data) => PoolMutexGuard { pool: self, data: Some(data) }
        }
    }

    fn put(&self, data: T) {
        let mut stack = self.stack.lock().unwrap();
        stack.push(data);
    }
}

impl<'a, T> Drop for PoolMutexGuard<'a, T> {
    fn drop(&mut self) {
        let data = self.data.take().unwrap();
        self.pool.put(data);
    }
}

struct PoolMpmc<T: Send + 'static> {
    stack: ArrayQueue<T>,
    create: CreateFn<T>,
}

struct PoolMpmcGuard<'a, T: Send + 'static> {
    pool: &'a PoolMpmc<T>,
    data: Option<T>,
}

impl<T: Send + 'static> PoolMpmc<T> {
    fn new(create: CreateFn<T>) -> PoolMpmc<T> {
        PoolMpmc { stack: ArrayQueue::with_capacity(1), create: create }
    }

    fn get(&self) -> PoolMpmcGuard<T> {
        match self.stack.pop() {
            None => {
                PoolMpmcGuard { pool: self, data: Some((self.create)()) }
            }
            Some(data) => PoolMpmcGuard { pool: self, data: Some(data) }
        }
    }

    fn put(&self, data: T) {
        let _ = self.stack.push(data);
    }
}

impl<'a, T: Send + 'static> Drop for PoolMpmcGuard<'a, T> {
    fn drop(&mut self) {
        let data = self.data.take().unwrap();
        self.pool.put(data);
    }
}

struct PoolLossyMpmc<T: Send + 'static> {
    pool: [Option<Box<T>>;16],
    index: AtomicUsize,
    create: CreateFn<Box<T>>,
}

struct PoolLossyMpmcGuard<'a, T: Send + 'static> {
    pool: &'a PoolLossyMpmc<T>,
    data: Option<Box<T>>,
}

impl<T: Send + 'static> PoolLossyMpmc<T> {
    fn new(create: CreateFn<Box<T>>) -> PoolLossyMpmc<T> {
        PoolLossyMpmc {
            pool: unsafe { mem::zeroed() },
            index: AtomicUsize::new(0),
            create: create,
        }
    }

    fn get(&self) -> PoolLossyMpmcGuard<T> {        
        let index = self.index.load(Ordering::Relaxed);
        let atomic: &AtomicUsize = unsafe { mem::transmute(self.pool[index..].as_ptr()) };
        let boxed: Option<Box<T>> = unsafe { mem::transmute(atomic.swap(0, Ordering::Acquire)) };
        let boxed = match boxed {
            Some(boxed) => boxed,
            None => (self.create)(),
        };
        if index > 0 { self.index.store(index - 1, Ordering::Relaxed); }
        PoolLossyMpmcGuard{ pool: &self, data: Some(boxed) }
    }

    fn put(&self, mut data: Box<T>) {
        let original = self.index.load(Ordering::Relaxed);
        let mut index = original;
        loop {
            let atomic: &AtomicPtr<T> = unsafe { mem::transmute(self.pool[index..].as_ptr()) };
            let boxed: Option<Box<T>> = unsafe { mem::transmute(atomic.swap(Box::into_raw(data), Ordering::Release)) };
            data = match boxed {
                Some(data) => data,
                None => break,
            };
            index += 1;
            if 15 <= index { break; }
        }
        if original != index {
            self.index.store(index, Ordering::Relaxed);
        }
    }
}

impl<'a, T: Send + 'static> Drop for PoolLossyMpmcGuard<'a, T> {
    fn drop(&mut self) {
        let data = self.data.take().unwrap();
        self.pool.put(data);
    }
}

struct PoolTreiber<T> {
    stack: TreiberStack<T>,
    create: CreateFn<T>,
}

struct PoolTreiberGuard<'a, T: 'a> {
    pool: &'a PoolTreiber<T>,
    data: Option<T>,
}

impl<T> PoolTreiber<T> {
    fn new(create: CreateFn<T>) -> PoolTreiber<T> {
        PoolTreiber { stack: TreiberStack::new(), create: create }
    }

    fn get(&self) -> PoolTreiberGuard<T> {
        match self.stack.pop() {
            None => {
                PoolTreiberGuard { pool: self, data: Some((self.create)()) }
            }
            Some(data) => PoolTreiberGuard { pool: self, data: Some(data) }
        }
    }

    fn put(&self, data: T) {
        self.stack.push(data);
    }
}

impl<'a, T> Drop for PoolTreiberGuard<'a, T> {
    fn drop(&mut self) {
        let data = self.data.take().unwrap();
        self.pool.put(data);
    }
}

struct PoolMs<T> {
    stack: MsQueue<T>,
    create: CreateFn<T>,
}

struct PoolMsGuard<'a, T: 'a> {
    pool: &'a PoolMs<T>,
    data: Option<T>,
}

impl<T> PoolMs<T> {
    fn new(create: CreateFn<T>) -> PoolMs<T> {
        PoolMs { stack: MsQueue::new(), create: create }
    }

    fn get(&self) -> PoolMsGuard<T> {
        match self.stack.try_pop() {
            None => {
                PoolMsGuard { pool: self, data: Some((self.create)()) }
            }
            Some(data) => PoolMsGuard { pool: self, data: Some(data) }
        }
    }

    fn put(&self, data: T) {
        self.stack.push(data);
    }
}

impl<'a, T> Drop for PoolMsGuard<'a, T> {
    fn drop(&mut self) {
        let data = self.data.take().unwrap();
        self.pool.put(data);
    }
}

struct PoolSeg<T> {
    stack: SegQueue<T>,
    create: CreateFn<T>,
}

struct PoolSegGuard<'a, T: 'a> {
    pool: &'a PoolSeg<T>,
    data: Option<T>,
}

impl<T> PoolSeg<T> {
    fn new(create: CreateFn<T>) -> PoolSeg<T> {
        PoolSeg { stack: SegQueue::new(), create: create }
    }

    fn get(&self) -> PoolSegGuard<T> {
        match self.stack.try_pop() {
            None => {
                PoolSegGuard { pool: self, data: Some((self.create)()) }
            }
            Some(data) => PoolSegGuard { pool: self, data: Some(data) }
        }
    }

    fn put(&self, data: T) {
        self.stack.push(data);
    }
}

impl<'a, T> Drop for PoolSegGuard<'a, T> {
    fn drop(&mut self) {
        let data = self.data.take().unwrap();
        self.pool.put(data);
    }
}
