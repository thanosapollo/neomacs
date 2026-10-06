//! The model GPU budget (design §4.4): its own pool, separate from the
//! shared media budget, never evicted. Every allocation is admitted against
//! the remaining budget *before* the resources are created; a resize
//! reserves the new targets while the old ones still exist (the overlap is
//! what is reserved) and releases the old ones afterwards.

use std::collections::HashMap;

/// Default pool size, overridable with `NEOMACS_MODEL_BUDGET_MB`.
pub const DEFAULT_MODEL_BUDGET_MB: u64 = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelBudget {
    limit: u64,
    used: u64,
    reserved: HashMap<u32, u64>,
}

impl ModelBudget {
    pub fn with_limit(limit: u64) -> Self {
        Self {
            limit,
            used: 0,
            reserved: HashMap::new(),
        }
    }

    /// The pool from `NEOMACS_MODEL_BUDGET_MB` (MiB), or the default.
    pub fn from_env() -> Self {
        let mb = std::env::var("NEOMACS_MODEL_BUDGET_MB")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|mb| *mb > 0)
            .unwrap_or(DEFAULT_MODEL_BUDGET_MB);
        Self::with_limit(mb.saturating_mul(1024 * 1024))
    }

    pub fn used(&self) -> u64 {
        self.used
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn reserved(&self, id: u32) -> u64 {
        self.reserved.get(&id).copied().unwrap_or(0)
    }

    /// Admit `bytes` more for model `id`, or refuse without changing
    /// anything.
    pub fn reserve(&mut self, id: u32, bytes: u64) -> Result<(), String> {
        let after = self.used.saturating_add(bytes);
        if after > self.limit {
            const MIB: f64 = 1024.0 * 1024.0;
            return Err(format!(
                "model needs {:.1} MiB; the model budget has {:.1} of {:.0} MiB left",
                bytes as f64 / MIB,
                self.limit.saturating_sub(self.used) as f64 / MIB,
                self.limit as f64 / MIB
            ));
        }
        self.used = after;
        *self.reserved.entry(id).or_default() += bytes;
        Ok(())
    }

    /// Return `bytes` of `id`'s reservation (never more than it holds).
    pub fn release(&mut self, id: u32, bytes: u64) {
        let Some(held) = self.reserved.get_mut(&id) else {
            return;
        };
        let bytes = bytes.min(*held);
        *held -= bytes;
        self.used -= bytes;
        if *held == 0 {
            self.reserved.remove(&id);
        }
    }

    /// Return everything `id` holds.
    pub fn release_all(&mut self, id: u32) {
        if let Some(held) = self.reserved.remove(&id) {
            self.used -= held;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Design §8.1: create → resize ×N → rescale → free returns to zero,
    /// with the peak overlap of a resize admitted only when it fits.
    #[test]
    fn create_resize_free_returns_to_zero() {
        let mut b = ModelBudget::with_limit(1000);
        b.reserve(1, 300).unwrap();
        for _ in 0..5 {
            // Resize: reserve new targets, then release the old ones.
            b.reserve(1, 200).unwrap();
            b.release(1, 200);
        }
        assert_eq!(b.used(), 300);
        b.reserve(2, 600).unwrap();
        // 300 + 600 + 200 > 1000: the overlap is refused, nothing changes.
        assert!(b.reserve(1, 200).is_err());
        assert_eq!((b.used(), b.reserved(1)), (900, 300));
        b.release_all(1);
        b.release_all(2);
        b.release_all(2);
        assert_eq!(b.used(), 0);
    }

    #[test]
    fn release_never_exceeds_the_reservation() {
        let mut b = ModelBudget::with_limit(100);
        b.reserve(1, 10).unwrap();
        b.reserve(2, 10).unwrap();
        b.release(1, 50);
        assert_eq!((b.used(), b.reserved(1), b.reserved(2)), (10, 0, 10));
    }

    #[test]
    fn refusal_names_the_sizes() {
        let mut b = ModelBudget::with_limit(2 * 1024 * 1024);
        let err = b.reserve(1, 3 * 1024 * 1024).unwrap_err();
        assert!(err.contains("3.0 MiB") && err.contains("of 2 MiB"), "{err}");
    }
}
