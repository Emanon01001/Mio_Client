//! Immutable vector publication without copying in render readers or holding
//! the publication lock during a native enumeration. Readers may retain a frame.
use std::sync::{Arc, Mutex};
pub(in crate::game) struct Snapshot<T> {
    published: Mutex<Arc<Vec<T>>>,
    spare: Mutex<Arc<Vec<T>>>,
}
impl<T> Snapshot<T> {
    pub fn new() -> Self {
        Self {
            published: Mutex::new(Arc::new(Vec::new())),
            spare: Mutex::new(Arc::new(Vec::new())),
        }
    }
    pub fn load(&self) -> Arc<Vec<T>> {
        Arc::clone(&self.published.lock().unwrap_or_else(|e| e.into_inner()))
    }
    pub fn publish(&self, update: impl FnOnce(&mut Vec<T>)) -> Option<usize> {
        let mut spare = self.spare.try_lock().ok()?;
        // A slow reader can still own the retired frame. Never mutate it and
        // never clone its soon-to-be-replaced elements just to obtain a writer.
        if Arc::get_mut(&mut spare).is_none() {
            *spare = Arc::new(Vec::with_capacity(spare.capacity()));
        }
        let data = Arc::get_mut(&mut spare).unwrap();
        update(data);
        let count = data.len();
        let mut published = self.published.try_lock().ok()?;
        std::mem::swap(&mut *published, &mut *spare);
        Some(count)
    }
    /// Only called once the producer hook has stopped. Existing readers remain valid.
    pub fn clear(&self) {
        *self.spare.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(Vec::new());
        *self.published.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(Vec::new());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_frames_stay_immutable_and_publication_does_not_lock_readers() {
        let s = Snapshot::new();
        s.publish(|v| v.extend([1, 2, 3])).unwrap();
        let old = s.load();
        s.publish(|v| {
            assert_eq!(&**s.load(), &[1, 2, 3]);
            v.clear();
            v.extend([4, 5]);
        })
        .unwrap();
        s.publish(|v| {
            v.clear();
            v.push(6);
        })
        .unwrap();
        assert_eq!(&**old, &[1, 2, 3]);
        assert_eq!(&**s.load(), &[6]);
        s.clear();
        assert!(s.load().is_empty());
        assert_eq!(&**old, &[1, 2, 3]);
    }
    #[test]
    fn concurrent_readers_see_whole_frames() {
        let s = Arc::new(Snapshot::new());
        let reader = Arc::clone(&s);
        let t = std::thread::spawn(move || {
            for _ in 0..5000 {
                let frame = reader.load();
                if let Some(first) = frame.first() {
                    assert!(frame.iter().all(|v| v == first));
                }
            }
        });
        for i in 0..2000 {
            s.publish(|v| {
                v.clear();
                v.resize(128, i);
            });
        }
        t.join().unwrap();
    }
}
