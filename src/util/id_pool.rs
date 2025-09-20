use std::{collections::BTreeSet, fmt::Debug};

use tokio::sync::watch;

/// A relatively naive ID pool implementation.
pub struct IdPool {
    max_id: u64,
    free_ids: BTreeSet<u64>,
    id_count: u64,
    id_count_watch_sender: watch::Sender<u64>,
    id_count_watch_receiver: watch::Receiver<u64>,
}

impl Debug for IdPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdPool")
            .field("max_id", &self.max_id)
            .field("free_ids", &self.free_ids)
            .field("id_count", &self.id_count)
            .finish()
    }
}

impl Default for IdPool {
    fn default() -> Self {
        let (id_count_watch_sender, id_count_watch_receiver) = watch::channel(0);
        Self {
            max_id: u64::MAX,
            free_ids: BTreeSet::new(),
            id_count: 0,
            id_count_watch_sender,
            id_count_watch_receiver,
        }
    }
}

//TODO: thiserror

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdPoolError {
    NoFreeIds,
    IdNotInUse(u64),
    IdOutOfBound(u64),
}

impl IdPool {
    pub fn new_with_max_limit(max_id: u64) -> Self {
        let (id_count_watch_sender, id_count_watch_receiver) = watch::channel(0);
        Self {
            max_id,
            free_ids: BTreeSet::new(),
            id_count: 0,
            id_count_watch_sender,
            id_count_watch_receiver,
        }
    }

    pub fn id_count(&self) -> u64 {
        self.id_count - self.free_ids.len() as u64
    }

    pub fn id_is_used(&self, id: u64) -> Result<bool, IdPoolError> {
        if id > self.max_id {
            Err(IdPoolError::IdOutOfBound(id))
        } else if self.free_ids.contains(&id) {
            Ok(false)
        } else {
            Ok(id <= self.id_count)
        }
    }

    pub fn request_id(&mut self) -> Result<u64, IdPoolError> {
        if !self.free_ids.is_empty() {
            let id = self.free_ids.pop_first().unwrap();
            self.update_watch();
            Ok(id)
        } else if self.id_count < self.max_id {
            self.id_count += 1;
            self.update_watch();
            Ok(self.id_count)
        } else {
            Err(IdPoolError::NoFreeIds)
        }
    }

    pub fn release_id(&mut self, id: u64) -> Result<(), IdPoolError> {
        if !self.id_is_used(id)? {
            Err(IdPoolError::IdNotInUse(id))
        } else {
            self.free_ids.insert(id);
            self.update_watch();
            Ok(())
        }
    }

    fn update_watch(&self) {
        self.id_count_watch_sender.send(self.id_count()).unwrap();
    }

    pub fn get_id_count_watch_receiver(&self) -> watch::Receiver<u64> {
        self.id_count_watch_receiver.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_pool() {
        let mut pool = IdPool::new_with_max_limit(10);
        assert_eq!(pool.request_id(), Ok(1));
        assert_eq!(pool.request_id(), Ok(2));
        assert_eq!(pool.request_id(), Ok(3));
        assert_eq!(pool.request_id(), Ok(4));
        assert_eq!(pool.id_count(), 4);
        assert_eq!(pool.request_id(), Ok(5));
        assert_eq!(pool.request_id(), Ok(6));
        assert_eq!(pool.request_id(), Ok(7));
        assert_eq!(pool.request_id(), Ok(8));
        assert_eq!(pool.request_id(), Ok(9));
        assert_eq!(pool.request_id(), Ok(10));
        assert_eq!(pool.id_count(), 10);
        assert_eq!(pool.request_id(), Err(IdPoolError::NoFreeIds));
        assert_eq!(pool.release_id(5), Ok(()));
        assert_eq!(pool.release_id(5), Err(IdPoolError::IdNotInUse(5)));
        assert_eq!(pool.id_count(), 9);
        assert_eq!(pool.request_id(), Ok(5));
        assert_eq!(pool.release_id(11), Err(IdPoolError::IdOutOfBound(11)));
    }

    #[test]
    fn test_id_pool_watch() {
        let mut pool = IdPool::new_with_max_limit(10);
        let receiver = pool.get_id_count_watch_receiver();

        assert_eq!(receiver.borrow().clone(), 0);
        pool.request_id().unwrap();
        assert_eq!(receiver.borrow().clone(), 1);
        pool.request_id().unwrap();
        assert_eq!(receiver.borrow().clone(), 2);
        pool.release_id(1).unwrap();
        assert_eq!(receiver.borrow().clone(), 1);
    }
}
