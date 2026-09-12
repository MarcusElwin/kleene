//! Ratings and curriculum. Tasks and solver configurations share one
//! Bradley-Terry scale: a solve is a win for the solver over the task, a
//! failure a win for the task. Both ratings move by the surprise of the
//! outcome (an Elo update, which is the online form of Bradley-Terry).

/// Rating points at which the odds are even.
pub const BASELINE: f64 = 1000.0;

/// Probability that a solver of rating `solver` solves a task of rating `task`.
pub fn solve_probability(solver: f64, task: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf((task - solver) / 400.0))
}

/// Step size: large while a party has few games, settling to 16.
pub fn k_factor(games: u64) -> f64 {
    if games < 5 {
        48.0
    } else if games < 20 {
        32.0
    } else {
        16.0
    }
}

/// Update both ratings after an attempt. Returns `(solver, task)`.
pub fn update(
    solver: f64,
    solver_games: u64,
    task: f64,
    task_games: u64,
    solved: bool,
) -> (f64, f64) {
    let p = solve_probability(solver, task);
    let outcome = if solved { 1.0 } else { 0.0 };
    let surprise = outcome - p;
    (
        solver + k_factor(solver_games) * surprise,
        task - k_factor(task_games) * surprise,
    )
}

/// Curriculum: among candidate `(id, task_rating)` pairs pick the one whose
/// solve probability for `solver` is closest to the learnability target
/// (0.5). Ties go to the earlier candidate (older tasks first).
pub fn pick_near_half(solver: f64, candidates: &[(String, f64)]) -> Option<String> {
    candidates
        .iter()
        .map(|(id, r)| (id, (solve_probability(solver, *r) - 0.5).abs()))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(id, _)| id.clone())
}

/// Step a generator's dial from its recent solve rate: up past 0.7, down
/// below 0.3, unchanged in between. Clamped to `[0, 1]`.
pub fn step_dial(dial: f64, solve_rate: f64, step: f64) -> f64 {
    let next = if solve_rate > 0.7 {
        dial + step
    } else if solve_rate < 0.3 {
        dial - step
    } else {
        dial
    };
    next.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_ratings_are_a_coin_flip_and_updates_are_symmetric() {
        assert!((solve_probability(1000.0, 1000.0) - 0.5).abs() < 1e-12);
        let (s, t) = update(1000.0, 0, 1000.0, 0, true);
        assert!(s > 1000.0 && t < 1000.0);
        assert!((s - 1000.0 - (1000.0 - t)).abs() < 1e-9);
        let (s2, t2) = update(1000.0, 0, 1000.0, 0, false);
        assert!(s2 < 1000.0 && t2 > 1000.0);
        // Beating a much harder task moves more than beating an easy one.
        let (hard, _) = update(1000.0, 30, 1400.0, 30, true);
        let (easy, _) = update(1000.0, 30, 600.0, 30, true);
        assert!(hard - 1000.0 > easy - 1000.0);
    }

    #[test]
    fn curriculum_prefers_the_task_nearest_even_odds() {
        let c = vec![
            ("easy".to_string(), 600.0),
            ("mid".to_string(), 1010.0),
            ("hard".to_string(), 1500.0),
        ];
        assert_eq!(pick_near_half(1000.0, &c).as_deref(), Some("mid"));
        assert_eq!(pick_near_half(1500.0, &c).as_deref(), Some("hard"));
        assert_eq!(pick_near_half(1000.0, &[]), None);
    }

    #[test]
    fn dial_steps_with_solve_rate() {
        assert!((step_dial(0.5, 0.9, 0.1) - 0.6).abs() < 1e-12);
        assert!((step_dial(0.5, 0.1, 0.1) - 0.4).abs() < 1e-12);
        assert!((step_dial(0.5, 0.5, 0.1) - 0.5).abs() < 1e-12);
        assert_eq!(step_dial(1.0, 1.0, 0.1), 1.0);
    }
}
