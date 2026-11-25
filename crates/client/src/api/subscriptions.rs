//! Keep failed native cleanup reachable until Runtime::shutdown can retry it.
use mio_sdk::{Error, Result};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub(super) trait Release: Send {
    fn release(&mut self) -> bool;
}
struct Entry<R: Release> {
    resource: Mutex<R>,
    closed: AtomicBool,
    released: AtomicBool,
}
impl<R: Release> Entry<R> {
    fn close(&self) -> bool {
        self.closed.store(true, Ordering::Release);
        let mut resource = self.resource.lock().unwrap_or_else(|e| e.into_inner());
        if !self.released.load(Ordering::Acquire) && resource.release() {
            self.released.store(true, Ordering::Release);
        }
        self.released.load(Ordering::Acquire)
    }
}
pub(super) struct Lease<R: Release>(Arc<Entry<R>>);
impl<R: Release> Lease<R> {
    pub fn open(&self) -> bool {
        !self.0.closed.load(Ordering::Acquire)
    }
}
impl<R: Release> Drop for Lease<R> {
    fn drop(&mut self) {
        self.0.close();
    }
}
struct State<R: Release> {
    closing: bool,
    entries: Vec<Arc<Entry<R>>>,
}
pub(super) struct Pool<R: Release>(Mutex<State<R>>);
impl<R: Release> Pool<R> {
    pub fn new() -> Self {
        Self(Mutex::new(State {
            closing: false,
            entries: Vec::new(),
        }))
    }
    pub fn acquire(
        &self,
        resource: R,
        prepare: impl FnOnce(&mut R) -> Result<()>,
    ) -> Result<Lease<R>> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.closing {
            return Err(Error::Detached);
        }
        state
            .entries
            .retain(|e| !e.released.load(Ordering::Acquire));
        if state.entries.len() >= 128 {
            return Err(Error::QueueFull);
        }
        let entry = Arc::new(Entry {
            resource: Mutex::new(resource),
            closed: AtomicBool::new(false),
            released: AtomicBool::new(false),
        });
        // Register before prepare, so even partial acquisition with a failed
        // rollback remains owned by shutdown instead of leaking a hook user.
        state.entries.push(Arc::clone(&entry));
        let result = prepare(&mut entry.resource.lock().unwrap_or_else(|e| e.into_inner()));
        if let Err(error) = result {
            entry.close();
            return Err(error);
        }
        Ok(Lease(entry))
    }
    pub fn close(&self) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.closing = true;
        state.entries.retain(|entry| !entry.close());
        state.entries.is_empty()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    struct Fake {
        releases: Arc<AtomicUsize>,
        failures: usize,
    }
    impl Release for Fake {
        fn release(&mut self) -> bool {
            self.releases.fetch_add(1, Ordering::SeqCst);
            if self.failures > 0 {
                self.failures -= 1;
                false
            } else {
                true
            }
        }
    }
    #[test]
    fn late_drop_and_repeated_shutdown_do_not_release_twice() {
        let releases = Arc::new(AtomicUsize::new(0));
        let pool = Pool::new();
        let lease = pool
            .acquire(
                Fake {
                    releases: releases.clone(),
                    failures: 0,
                },
                |_| Ok(()),
            )
            .unwrap();
        assert!(lease.open());
        assert!(pool.close());
        assert!(!lease.open());
        drop(lease);
        assert!(pool.close());
        assert_eq!(releases.load(Ordering::SeqCst), 1);
        assert!(matches!(
            pool.acquire(
                Fake {
                    releases,
                    failures: 0
                },
                |_| Ok(())
            ),
            Err(Error::Detached)
        ));
    }
    #[test]
    fn failed_drop_cleanup_is_retained_and_retried() {
        let releases = Arc::new(AtomicUsize::new(0));
        let pool = Pool::new();
        let lease = pool
            .acquire(
                Fake {
                    releases: releases.clone(),
                    failures: 2,
                },
                |_| Ok(()),
            )
            .unwrap();
        drop(lease);
        assert!(!pool.close());
        assert!(pool.close());
        assert_eq!(releases.load(Ordering::SeqCst), 3);
    }
    #[test]
    fn partial_acquisition_failure_stays_registered_until_cleaned() {
        let releases = Arc::new(AtomicUsize::new(0));
        let pool = Pool::new();
        assert!(matches!(
            pool.acquire(
                Fake {
                    releases: releases.clone(),
                    failures: 1
                },
                |_| Err(Error::InvalidValue)
            ),
            Err(Error::InvalidValue)
        ));
        assert!(pool.close());
        assert_eq!(releases.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn dropped_leases_do_not_exhaust_capacity() {
        let releases = Arc::new(AtomicUsize::new(0));
        let pool = Pool::new();
        for _ in 0..256 {
            drop(
                pool.acquire(
                    Fake {
                        releases: releases.clone(),
                        failures: 0,
                    },
                    |_| Ok(()),
                )
                .unwrap(),
            );
        }
        let leases: Vec<_> = (0..128)
            .map(|_| {
                pool.acquire(
                    Fake {
                        releases: releases.clone(),
                        failures: 0,
                    },
                    |_| Ok(()),
                )
                .unwrap()
            })
            .collect();
        assert!(matches!(
            pool.acquire(
                Fake {
                    releases,
                    failures: 0
                },
                |_| Ok(())
            ),
            Err(Error::QueueFull)
        ));
        drop(leases);
        assert!(pool.close());
    }
}
