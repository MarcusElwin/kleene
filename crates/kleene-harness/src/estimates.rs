//! What the planner learns from actuals: selectivities and branching factors
//! keyed by the function's template (the `estimates` table), and cascade
//! thresholds chosen from a sample (`CALIBRATE`, the `calibrations` table).

use kleene_sql::FunctionBody;

/// A stable 64-bit hash of a function's template, as hex: the key the
/// learned tables use, so a refined prompt starts its own history and the
/// same prompt under another name shares one. FNV-1a, so the value does
/// not depend on the Rust version.
pub fn template_hash(name: &str, body: &FunctionBody) -> String {
    let text = match body {
        FunctionBody::Prompt { template, .. } => format!("prompt:{template}"),
        FunctionBody::Sql { query } => format!("sql:{query}"),
        FunctionBody::Shell { command } => format!("shell:{command}"),
    };
    fnv1a(&format!("{}\n{text}", name.to_ascii_lowercase()))
}

/// The key for a builtin table function (`expand`, a tool): its name.
pub fn builtin_hash(name: &str) -> String {
    fnv1a(&format!("builtin:{}", name.to_ascii_lowercase()))
}

fn fnv1a(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// What a calibration decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// Scores below this are rejected without asking the oracle.
    pub low: f64,
    /// Scores at or above this are accepted without asking the oracle.
    pub high: f64,
    /// Share of the rows the proxy accepts outright that the oracle agreed with.
    pub precision: f64,
    /// Share of the oracle's positives the band and the accepted rows keep.
    pub recall: f64,
    /// Sampled rows accepted outright, asked (in the band) and rejected.
    pub accepted: usize,
    /// Rows in the band.
    pub asked: usize,
    /// Rows rejected outright.
    pub rejected: usize,
}

/// Choose cascade thresholds from `samples` of `(proxy score, oracle
/// verdict)`, the LOTUS way: `high` is the lowest score at which the rows
/// scoring at least that much are oracle-true in at least `precision` of
/// cases (1.0, accepting nothing outright, when no score achieves it);
/// `low` is the highest score at which the rows scoring at least that much
/// still hold `recall` of the oracle's positives (0.0, rejecting nothing,
/// when the sample has no positives). `low` never exceeds `high`.
pub fn choose_thresholds(samples: &[(f64, bool)], recall: f64, precision: f64) -> Thresholds {
    let positives = samples.iter().filter(|(_, ok)| *ok).count();
    let mut scores: Vec<f64> = samples.iter().map(|(s, _)| *s).collect();
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    scores.dedup();
    let at_least = |t: f64| samples.iter().filter(move |(s, _)| *s >= t);
    let high = scores
        .iter()
        .copied()
        .find(|t| {
            let accepted = at_least(*t).count();
            let right = at_least(*t).filter(|(_, ok)| *ok).count();
            accepted > 0 && right as f64 / accepted as f64 + 1e-9 >= precision
        })
        .unwrap_or(1.0);
    let low = if positives == 0 {
        0.0
    } else {
        scores
            .iter()
            .rev()
            .copied()
            .find(|t| {
                let kept = at_least(*t).filter(|(_, ok)| *ok).count();
                kept as f64 / positives as f64 + 1e-9 >= recall
            })
            .unwrap_or(0.0)
    };
    let low = low.min(high);
    let accepted = at_least(high).count();
    let right = at_least(high).filter(|(_, ok)| *ok).count();
    let kept = at_least(low).filter(|(_, ok)| *ok).count();
    Thresholds {
        low,
        high,
        precision: if accepted == 0 {
            1.0
        } else {
            right as f64 / accepted as f64
        },
        recall: if positives == 0 {
            1.0
        } else {
            kept as f64 / positives as f64
        },
        accepted,
        asked: samples
            .iter()
            .filter(|(s, _)| *s >= low && *s < high)
            .count(),
        rejected: samples.iter().filter(|(s, _)| *s < low).count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_stable_and_distinguish_templates() {
        let a = template_hash(
            "rel",
            &FunctionBody::Prompt {
                template: "Is {t} relevant?".into(),
                batch: None,
            },
        );
        let b = template_hash(
            "REL",
            &FunctionBody::Prompt {
                template: "Is {t} relevant?".into(),
                batch: Some(20),
            },
        );
        assert_eq!(
            a, b,
            "the name's case and the batch size are not part of the key"
        );
        assert_eq!(a, "b0180a445f067c26");
        let c = template_hash(
            "rel",
            &FunctionBody::Prompt {
                template: "Is {t} relevant to the question?".into(),
                batch: None,
            },
        );
        assert_ne!(a, c);
        assert_ne!(builtin_hash("expand"), builtin_hash("rlm"));
    }

    #[test]
    fn thresholds_meet_the_targets_on_the_sample() {
        // Scores rise with the oracle's verdicts, with one noisy row at 0.7.
        let samples = [
            (0.1, false),
            (0.2, false),
            (0.3, false),
            (0.4, true),
            (0.5, false),
            (0.6, true),
            (0.7, false),
            (0.8, true),
            (0.9, true),
            (1.0, true),
        ];
        let t = choose_thresholds(&samples, 0.9, 0.9);
        // Every positive scores >= 0.4, and 0.8 is the first score at which
        // the rows at or above it are all true.
        assert_eq!((t.low, t.high), (0.4, 0.8));
        assert_eq!((t.accepted, t.asked, t.rejected), (3, 4, 3));
        assert!((t.precision - 1.0).abs() < 1e-9 && (t.recall - 1.0).abs() < 1e-9);
        // A looser precision target accepts from 0.6 (3 of 4 true = 0.75).
        let t = choose_thresholds(&samples, 0.9, 0.75);
        assert_eq!(t.high, 0.6);
        // Recall 0.8 may drop one of the five positives: low rises to 0.6.
        let t = choose_thresholds(&samples, 0.8, 0.9);
        assert_eq!(t.low, 0.6);
        assert!((t.recall - 0.8).abs() < 1e-9);
    }

    #[test]
    fn degenerate_samples_fall_back_safely() {
        // No positives: nothing is rejected, nothing accepted outright.
        let t = choose_thresholds(&[(0.2, false), (0.9, false)], 0.9, 0.9);
        assert_eq!((t.low, t.high), (0.0, 1.0));
        assert_eq!((t.accepted, t.asked, t.rejected), (0, 2, 0));
        // All positives: everything from the lowest score up is accepted.
        let t = choose_thresholds(&[(0.2, true), (0.9, true)], 0.9, 0.9);
        assert_eq!((t.low, t.high), (0.2, 0.2));
        assert_eq!(t.accepted, 2);
        // Empty sample.
        let t = choose_thresholds(&[], 0.9, 0.9);
        assert_eq!((t.low, t.high), (0.0, 1.0));
    }
}
