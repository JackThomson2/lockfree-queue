use std::ptr;
use std::sync::atomic::Ordering::{Acquire, Relaxed};

use flize::{Atomic, Collector, Shared, Shield};

pub struct Stack<T> {
    head: Atomic<Node<T>>,
    collector: Collector,
}

unsafe impl<T> Send for Stack<T> {}

unsafe impl<T> Sync for Stack<T> {}

struct Node<T> {
    data: T,
    next: Atomic<Node<T>>,
}

impl<T> Stack<T> {
    pub fn new() -> Stack<T> {
        Stack {
            head: Atomic::null(),
            collector: Collector::new(),
        }
    }

    pub fn pop(&self) -> Option<T> {
        let guard = self.collector.thin_shield();

        loop {
            unsafe {
                let head = self.head.load(Acquire, &guard);
                if head.is_null() {
                    return None;
                }

                let next = head.as_ref_unchecked().next.load(Relaxed, &guard);

                // if snapshot is still good, update from `head` to `next`
                if self
                    .head
                    .compare_exchange(head, next, Acquire, Relaxed, &guard)
                    .is_ok()
                {
                    guard.retire(move || drop(head));
                    // extract out the data from the now-unlinked node
                    return Some(ptr::read(&(*head.as_ref_unchecked()).data));
                }
            }
        }
    }

    pub fn push(&self, t: T) {
        // allocate the node, and immediately turn it into a *mut pointer
        let guard = self.collector.thin_shield();

        let mut n = unsafe {
            Shared::from_ptr(Box::into_raw(Box::new(Node {
                data: t,
                next: Atomic::null(),
            })))
        };
        loop {
            // snapshot current head
            let head = self.head.load(Relaxed, &guard);

            // update `next` pointer with snapshot
            unsafe {
                n.as_ref_unchecked().next.store(head, Relaxed);
            }

            // if snapshot is still good, link in new node
            match self
                .head
                .compare_exchange(head, n, Acquire, Relaxed, &guard)
            {
                Ok(_) => return,
                Err(owned) => n = owned,
            }
        }
    }
}

#[test]
fn push_items() {
    let stack = Stack::new();

    stack.push(10);
    stack.push(5);
    stack.push(1);

    assert_eq!(stack.pop().unwrap(), 1);
    assert_eq!(stack.pop().unwrap(), 5);
    assert_eq!(stack.pop().unwrap(), 10);
}

#[test]
fn single_run() {
    use std::time;

    let stack = Stack::new();
    let now = time::Instant::now();

    const RUNS: i32 = 10_000_000;

    for _i in 0..RUNS {
        stack.push(11);
    }

    for _i in 0..RUNS {
        assert_eq!(stack.pop().unwrap(), 11);
    }

    println!(
        "It took {:?} to write and read {} messages",
        now.elapsed(),
        RUNS
    );
}

#[test]
fn thread_test() {
    use std::sync::Arc;
    use std::{thread, time};

    const RUNS: i32 = 1;

    let stack = Arc::new(Stack::new());

    let now = time::Instant::now();

    let our_copy = stack.clone();
    thread::spawn(move || {
        for _i in 0..RUNS {
            our_copy.push(1);
        }
    });

    let our_copy = stack.clone();
    thread::spawn(move || {
        for _i in 0..RUNS {
            our_copy.push(1);
        }
    });

    let wait = time::Duration::from_millis(1);
    thread::sleep(wait);

    let mut count = 0;
    for _ in 0..RUNS * 2 {
        count += stack.pop().unwrap();
    }

    println!(
        "It took {:?} to write and read {} messages",
        now.elapsed(),
        RUNS * 2
    );

    assert_eq!(count, RUNS * 2)
}
