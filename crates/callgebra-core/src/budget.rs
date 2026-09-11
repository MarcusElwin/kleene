//! Budgets bound what a session or statement may spend.
//!
//! A budget is inherited downward: a child session receives a *slice* of its
//! parent's remaining budget and can never exceed it. Every dimension is
//! optional; `None` means unbounded in that dimension.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

/// Limits for calls, tokens, dollars, recursion depth and wall clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Budget {
    /// Maximum model calls.
    pub calls: Option<u64>,
    /// Maximum total tokens (input plus output).
    pub tokens: Option<u64>,
    /// Maximum spend in USD.
    pub dollars: Option<f64>,
    /// Maximum recursion depth (root session is depth 0).
    pub max_depth: Option<u32>,
    /// Maximum wall-clock time.
    pub wall: Option<Duration>,
}

impl Budget {
    /// A budget with no limits.
    pub fn unbounded() -> Self {
        Self::default()
    }

    /// The budget left after `used` has been spent.
    pub fn remaining(&self, used: &BudgetUsage) -> Budget {
        Budget {
            calls: self.calls.map(|c| c.saturating_sub(used.calls)),
            tokens: self.tokens.map(|t| t.saturating_sub(used.tokens)),
            dollars: self.dollars.map(|d| (d - used.dollars).max(0.0)),
            max_depth: self.max_depth,
            wall: self.wall.map(|w| w.saturating_sub(used.wall)),
        }
    }

    /// A fraction of this budget, for a child session. Depth is reduced by
    /// one so children cannot recurse past the parent's cap.
    pub fn slice(&self, fraction: f64) -> Budget {
        let f = fraction.clamp(0.0, 1.0);
        Budget {
            calls: self.calls.map(|c| ((c as f64) * f).floor() as u64),
            tokens: self.tokens.map(|t| ((t as f64) * f).floor() as u64),
            dollars: self.dollars.map(|d| d * f),
            max_depth: self.max_depth.map(|d| d.saturating_sub(1)),
            wall: self.wall.map(|w| w.mul_f64(f)),
        }
    }

    /// Check whether `used` fits inside this budget.
    pub fn check(&self, used: &BudgetUsage) -> Result<(), BudgetExceeded> {
        if let Some(c) = self.calls {
            if used.calls > c {
                return Err(BudgetExceeded::Calls {
                    limit: c,
                    used: used.calls,
                });
            }
        }
        if let Some(t) = self.tokens {
            if used.tokens > t {
                return Err(BudgetExceeded::Tokens {
                    limit: t,
                    used: used.tokens,
                });
            }
        }
        if let Some(d) = self.dollars {
            if used.dollars > d {
                return Err(BudgetExceeded::Dollars {
                    limit: d,
                    used: used.dollars,
                });
            }
        }
        if let Some(w) = self.wall {
            if used.wall > w {
                return Err(BudgetExceeded::Wall {
                    limit: w,
                    used: used.wall,
                });
            }
        }
        Ok(())
    }
}

/// What has been spent so far.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct BudgetUsage {
    /// Model calls made.
    pub calls: u64,
    /// Tokens consumed (input plus output).
    pub tokens: u64,
    /// Dollars spent.
    pub dollars: f64,
    /// Wall-clock time elapsed.
    pub wall: Duration,
}

impl BudgetUsage {
    /// Add another usage record to this one.
    pub fn add(&mut self, other: &BudgetUsage) {
        self.calls += other.calls;
        self.tokens += other.tokens;
        self.dollars += other.dollars;
        self.wall += other.wall;
    }
}

/// A budget dimension was exceeded.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum BudgetExceeded {
    /// Too many calls.
    #[error("call budget exceeded: {used} > {limit}")]
    Calls {
        /// The limit.
        limit: u64,
        /// What was used.
        used: u64,
    },
    /// Too many tokens.
    #[error("token budget exceeded: {used} > {limit}")]
    Tokens {
        /// The limit.
        limit: u64,
        /// What was used.
        used: u64,
    },
    /// Too much spend.
    #[error("dollar budget exceeded: {used:.4} > {limit:.4}")]
    Dollars {
        /// The limit.
        limit: f64,
        /// What was used.
        used: f64,
    },
    /// Recursion too deep.
    #[error("depth {depth} exceeds max depth {limit}")]
    Depth {
        /// The limit.
        limit: u32,
        /// Attempted depth.
        depth: u32,
    },
    /// Out of time.
    #[error("wall-clock budget exceeded: {used:?} > {limit:?}")]
    Wall {
        /// The limit.
        limit: Duration,
        /// What was used.
        used: Duration,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_shrinks_every_dimension_and_depth() {
        let b = Budget {
            calls: Some(100),
            tokens: Some(1000),
            dollars: Some(2.0),
            max_depth: Some(2),
            wall: Some(Duration::from_secs(100)),
        };
        let s = b.slice(0.25);
        assert_eq!(s.calls, Some(25));
        assert_eq!(s.tokens, Some(250));
        assert_eq!(s.dollars, Some(0.5));
        assert_eq!(s.max_depth, Some(1));
        assert_eq!(s.wall, Some(Duration::from_secs(25)));
    }

    #[test]
    fn check_reports_the_first_exceeded_dimension() {
        let b = Budget {
            calls: Some(1),
            ..Budget::default()
        };
        let used = BudgetUsage {
            calls: 2,
            ..BudgetUsage::default()
        };
        assert_eq!(
            b.check(&used),
            Err(BudgetExceeded::Calls { limit: 1, used: 2 })
        );
        assert_eq!(b.remaining(&used).calls, Some(0));
    }
}
