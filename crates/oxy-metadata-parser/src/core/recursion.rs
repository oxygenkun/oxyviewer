//! Recursion guard for detecting circular references (F6).

use std::collections::HashSet;

use crate::core::Error;

/// Tracks visited offsets to detect circular structures (IFD chains, PDF object refs).
pub struct RecursionGuard {
    visited: HashSet<u64>,
    max_depth: usize,
}

impl RecursionGuard {
    /// Create a new guard with the given maximum depth.
    pub fn new(max_depth: usize) -> Self {
        Self {
            visited: HashSet::new(),
            max_depth,
        }
    }

    /// Mark an offset as visited. Returns `Err(Error::Cycle)` if already visited
    /// or if the maximum depth has been exceeded.
    pub fn enter(&mut self, offset: u64) -> crate::core::Result<()> {
        if self.visited.len() >= self.max_depth {
            return Err(Error::Format(format!(
                "recursion depth limit ({}) exceeded",
                self.max_depth
            )));
        }
        if !self.visited.insert(offset) {
            return Err(Error::Cycle(offset));
        }
        Ok(())
    }

    /// Remove an offset from the visited set (for backtracking).
    pub fn leave(&mut self, offset: u64) {
        self.visited.remove(&offset);
    }

    /// Check if an offset has been visited without marking it.
    pub fn contains(&self, offset: u64) -> bool {
        self.visited.contains(&offset)
    }

    /// Number of offsets currently tracked.
    pub fn depth(&self) -> usize {
        self.visited.len()
    }

    /// Reset the guard, clearing all visited offsets.
    pub fn reset(&mut self) {
        self.visited.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_and_detect_cycle() {
        let mut g = RecursionGuard::new(100);
        g.enter(100).unwrap();
        g.enter(200).unwrap();
        let err = g.enter(100).unwrap_err();
        assert!(matches!(err, Error::Cycle(100)));
    }

    #[test]
    fn max_depth() {
        let mut g = RecursionGuard::new(2);
        g.enter(1).unwrap();
        g.enter(2).unwrap();
        let err = g.enter(3).unwrap_err();
        assert!(matches!(err, Error::Format(_)));
    }

    #[test]
    fn leave_allows_reentry() {
        let mut g = RecursionGuard::new(100);
        g.enter(42).unwrap();
        g.leave(42);
        g.enter(42).unwrap(); // should succeed
    }

    #[test]
    fn contains() {
        let mut g = RecursionGuard::new(100);
        assert!(!g.contains(10));
        g.enter(10).unwrap();
        assert!(g.contains(10));
    }

    #[test]
    fn reset() {
        let mut g = RecursionGuard::new(100);
        g.enter(1).unwrap();
        g.enter(2).unwrap();
        assert_eq!(g.depth(), 2);
        g.reset();
        assert_eq!(g.depth(), 0);
        g.enter(1).unwrap(); // should succeed after reset
    }
}
