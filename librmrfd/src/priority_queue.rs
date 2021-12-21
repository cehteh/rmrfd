//! A channel/message-queue based on a pairing heap with some special features:
//!
//!  * Tracks processing of messages with a ReceiveGuards and a counter. This is
//!    useful when the recevier itself may send new items to the queue. As long as any
//!    receiver is processing items, others 'recv()' are blocking. Once the queue becomes
//!    empty and the final item is processed one waiter will get a notification with a
//!    'Drained' message.
// PLANNED: * Use without contention by pushing items to a local VecDeque and once the lock
//             becomes available merge the local data with the main queue with
//             'try_merge_send()'.
//!
use std::sync::{Condvar, Mutex, TryLockError};
use std::ops::Deref;
use std::collections::BinaryHeap;
use std::sync::atomic::{self, AtomicBool, AtomicUsize};

#[allow(unused_imports)]
pub use log::{debug, error, info, trace, warn};

/// A queue which orders items by priority (smallest first)
#[derive(Debug)]
pub struct PriorityQueue<K, P>
where
    K: Send,
    P: PartialOrd + Default + Ord,
{
    heap:        Mutex<BinaryHeap<QueueEntry<K, P>>>,
    in_progress: AtomicUsize,
    is_drained:  AtomicBool,
    notify:      Condvar,
}

impl<K, P> Default for PriorityQueue<K, P>
where
    K: Send,
    P: PartialOrd + Default + Ord,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, P> PriorityQueue<K, P>
where
    K: Send,
    P: PartialOrd + Default + Ord,
{
    /// Create a new PriorityQueue
    pub fn new() -> PriorityQueue<K, P> {
        PriorityQueue {
            heap:        Mutex::new(BinaryHeap::new()),
            in_progress: AtomicUsize::new(0),
            is_drained:  AtomicBool::new(true),
            notify:      Condvar::new(),
        }
    }

    /// Pushes an item with some prio onto the queue.
    pub fn send(&self, item: K, prio: P) {
        self.in_progress.fetch_add(1, atomic::Ordering::SeqCst);
        self.is_drained.store(false, atomic::Ordering::SeqCst);
        self.heap
            .lock()
            .expect("Mutex not poisoned")
            .push(QueueEntry::Item(item, prio));
        self.notify.notify_one();
    }

    /// Send the 'Drained' message
    fn send_drained(&self) {
        if self
            .is_drained
            .compare_exchange(
                false,
                true,
                atomic::Ordering::SeqCst,
                atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            self.in_progress.fetch_add(1, atomic::Ordering::SeqCst);
            self.heap
                .lock()
                .expect("Mutex not poisoned")
                .push(QueueEntry::Drained);
            self.notify.notify_one();
        }
    }

    /// Returns the smallest item from a queue. This item is wraped in a ReceiveGuard/QueueEntry
    pub fn recv(&self) -> ReceiveGuard<K, P> {
        let entry = self
            .notify
            .wait_while(self.heap.lock().expect("Mutex not poisoned"), |heap| {
                heap.is_empty()
            })
            .expect("Mutex not poisoned")
            .pop()
            .unwrap();

        ReceiveGuard::new(entry, self)
    }

    /// Try to get the smallest item from a queue. Will return Some<ReceiveGuard> when an entry was available.
    pub fn try_recv(&self) -> Option<ReceiveGuard<K, P>> {
        match self.heap.try_lock() {
            Ok(mut heap) => heap.pop().map(|entry| ReceiveGuard::new(entry, self)),
            Err(TryLockError::WouldBlock) => None,
            _ => panic!("Poisoned Mutex"),
        }
    }

    // pub fn try_send(&self, item: K, prio: P) -> Option<K, P> {
    //     todo!()
    // }

    // pub fn merge(&self, PriorityQueue<K, P>)  {
    //     todo!()
    // }

    // pub fn try_merge(&self, PriorityQueue<K, P>) -> Option<PriorityQueue<K, P>> {
    //     todo!()
    // }

    // pub fn try_send_merge(&self, item: K, prio: P, PriorityQueue<K, P>)
}

/// Type for the received message
#[derive(Debug, Clone, Copy)]
pub enum QueueEntry<K: Send, P: Ord> {
    /// Entry with data K and priority P
    Item(K, P),
    /// Queue got empty and no other workers processing a ReceiveGuard
    Drained,
    /// Default value when taken from a ReceiveGuard
    Taken,
}

impl<K: Send, P: Ord> QueueEntry<K, P> {
    /// Returns a reference to the value of the item.
    pub fn entry(&self) -> Option<&K> {
        match &self {
            QueueEntry::Item(k, _) => Some(k),
            _ => None,
        }
    }

    /// Returns a reference to the priority of the item.
    pub fn priority(&self) -> Option<&P> {
        match &self {
            QueueEntry::Item(_, prio) => Some(prio),
            _ => None,
        }
    }

    /// Returns 'true' when the queue is drained
    pub fn is_drained(&self) -> bool {
        matches!(self, QueueEntry::Drained)
    }
}

impl<K: Send, P: Ord> Ord for QueueEntry<K, P> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (QueueEntry::Item(_, a), QueueEntry::Item(_, b)) => b.cmp(a),
            (QueueEntry::Drained, QueueEntry::Drained) => Ordering::Equal,
            (QueueEntry::Drained, _) => Ordering::Greater,
            (_, QueueEntry::Drained) => Ordering::Less,
            (_, _) => unreachable!("'Taken' should never appear here"),
        }
    }
}

impl<K: Send, P: Ord> PartialOrd for QueueEntry<K, P> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: Send, P: Ord> PartialEq for QueueEntry<K, P> {
    fn eq(&self, other: &Self) -> bool {
        use QueueEntry::*;
        match (self, other) {
            (Item(_, a), Item(_, b)) => a == b,
            (Drained, Drained) | (Taken, Taken) => true,
            (_, _) => false,
        }
    }
}

impl<K: Send, P: Ord> Eq for QueueEntry<K, P> {}

impl<K: Send, P: Ord> Default for QueueEntry<K, P> {
    fn default() -> Self {
        QueueEntry::Taken
    }
}

/// Wraps a QueueEntry, when dropped and the queue is empty it sends a final 'Drained' message
/// to notify that there is no further work in progress.
#[derive(Debug)]
pub struct ReceiveGuard<'a, K, P>
where
    K: Send,
    P: PartialOrd + Default + Ord,
{
    item: QueueEntry<K, P>,
    pq:   &'a PriorityQueue<K, P>,
}

impl<'a, K, P> ReceiveGuard<'a, K, P>
where
    K: Send,
    P: PartialOrd + Default + Ord,
{
    fn new(item: QueueEntry<K, P>, pq: &'a PriorityQueue<K, P>) -> Self {
        ReceiveGuard { item, pq }
    }

    /// Takes the 'QueueEntry' item out of a ReceiveGuard, drop the guard (and may by that send the 'Drained' message).
    pub fn into_item(mut self) -> QueueEntry<K, P> {
        std::mem::take(&mut self.item)
    }
}

impl<K, P> Deref for ReceiveGuard<'_, K, P>
where
    K: Send,
    P: PartialOrd + Default + Ord,
{
    type Target = QueueEntry<K, P>;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

impl<K, P> Drop for ReceiveGuard<'_, K, P>
where
    K: Send,
    P: PartialOrd + Default + Ord,
{
    fn drop(&mut self) {
        if self.pq.in_progress.fetch_sub(1, atomic::Ordering::SeqCst) == 1 {
            self.pq.send_drained()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::sync::Arc;

    use super::{PriorityQueue, QueueEntry};
    use crate::test;

    #[test]
    fn smoke() {
        test::init_env_logging();
        let queue: PriorityQueue<String, u64> = PriorityQueue::new();
        queue.send("test 1".to_string(), 1);
        queue.send("test 3".to_string(), 3);
        queue.send("test 2".to_string(), 2);
        assert_eq!(*queue.recv(), QueueEntry::Item("test 1".to_string(), 1));
        assert_eq!(*queue.recv(), QueueEntry::Item("test 2".to_string(), 2));
        assert_eq!(*queue.recv(), QueueEntry::Item("test 3".to_string(), 3));
        assert_eq!(*queue.recv(), QueueEntry::Drained);
        assert!(queue.try_recv().is_none());
    }

    #[test]
    fn try_recv() {
        test::init_env_logging();
        let queue: PriorityQueue<String, u64> = PriorityQueue::new();
        queue.send("test 1".to_string(), 1);
        queue.send("test 3".to_string(), 3);
        queue.send("test 2".to_string(), 2);
        assert!(queue.try_recv().is_some());
        assert!(queue.try_recv().is_some());
        assert!(queue.try_recv().is_some());
        assert!(queue.try_recv().is_some());
        assert!(queue.try_recv().is_none());
        assert!(queue.try_recv().is_none());
    }

    #[test]
    fn threads() {
        test::init_env_logging();
        let queue: Arc<PriorityQueue<String, u64>> = Arc::new(PriorityQueue::new());

        let thread1_queue = queue.clone();
        let thread1 = thread::spawn(move || {
            thread1_queue.send("test 1".to_string(), 1);
            thread1_queue.send("test 3".to_string(), 3);
            thread1_queue.send("test 2".to_string(), 2);
        });

        let thread2_queue = queue.clone();
        let thread2 = thread::spawn(move || {
            assert_eq!(
                *thread2_queue.recv(),
                QueueEntry::Item("test 1".to_string(), 1)
            );
            assert_eq!(
                *thread2_queue.recv(),
                QueueEntry::Item("test 2".to_string(), 2)
            );
            assert_eq!(
                *thread2_queue.recv(),
                QueueEntry::Item("test 3".to_string(), 3)
            );
            assert!(thread2_queue.recv().is_drained());
            assert!(thread2_queue.try_recv().is_none());
            thread2_queue.send("test 4".to_string(), 4);
            assert_eq!(
                *thread2_queue.recv(),
                QueueEntry::Item("test 4".to_string(), 4)
            );
            assert!(thread2_queue.recv().is_drained());
            assert!(thread2_queue.try_recv().is_none());
        });

        thread1.join().unwrap();
        thread2.join().unwrap();
    }
}
