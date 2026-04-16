//! test
//! =========
//!
//! Author: oghenemarho
//! Created: 16/04/2026
//! Project: async-rust-workspace
//!
//! Description:
//!
use std::ops::{Add, Coroutine, CoroutineState, Sub};
use std::pin::Pin;
use std::sync::{Arc, Mutex, TryLockResult};

pub struct MutexCoroutine {
    pub handle: Arc<Mutex<u8>>,
    pub threshold: u8,
}

impl Coroutine<()> for MutexCoroutine {
    type Yield = ();
    type Return = ();

    fn resume(mut self: Pin<&mut Self>, arg: ()) -> CoroutineState<Self::Yield, Self::Return> {
        match self.handle.try_lock() {
            Ok(mut handle) => *handle += 1,
            Err(_) => return CoroutineState::Yielded(()),
        };
        self.threshold -= 1;
        if self.threshold == 0 {
            return CoroutineState::Complete(());
        }
        CoroutineState::Yielded(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::{Context, Poll};

    //sync testing interface
    fn check_yield(coroutine: &mut MutexCoroutine) -> bool {
        match Pin::new(coroutine).resume(()) {
            CoroutineState::Yielded(_) => true,
            CoroutineState::Complete(_) => false,
        }
    }

    // async runtime interface
    impl Future for MutexCoroutine {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            match Pin::new(&mut self).resume(()) {
                CoroutineState::Yielded(()) => Poll::Ready(()),
                CoroutineState::Complete(()) => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        }
    }

    #[test]
    fn basic_test() {
        let handle = Arc::new(Mutex::new(0));
        let mut first = MutexCoroutine {
            handle: handle.clone(),
            threshold: 2,
        };
        let mut second = MutexCoroutine {
            handle: handle.clone(),
            threshold: 2,
        };
        // Since we have the lock here and the coroutine cannot obtain them,
        // the coroutine should yield and there should be no change in the lock value.
        let lock = handle.lock().unwrap();
        for _ in 0..3 {
            assert!(check_yield(&mut first));
            assert!(check_yield(&mut second));
        }

        assert_eq!(*lock, 0);
        std::mem::drop(lock); // release lock

        assert!(check_yield(&mut first));
        assert_eq!(*handle.lock().unwrap(), 1);

        assert!(check_yield(&mut second));
        assert_eq!(*handle.lock().unwrap(), 2);

        // it should return complete because threshold will be 0 now.
        assert!(!check_yield(&mut first));
        assert_eq!(*handle.lock().unwrap(), 3);

        assert!(!check_yield(&mut second));
        assert_eq!(*handle.lock().unwrap(), 4);
    }

    #[tokio::test]
    async fn async_test() {
        let handle = Arc::new(Mutex::new(0));
        let mut first = MutexCoroutine {
            handle: handle.clone(),
            threshold: 2,
        };
        let mut second = MutexCoroutine {
            handle: handle.clone(),
            threshold: 2,
        };
        let handle_one = tokio::spawn(first);
        let handle_two = tokio::spawn(second);

        handle_one.await.unwrap();
        handle_two.await.unwrap();
        assert_eq!(*handle.lock().unwrap(), 2);
    }
}
