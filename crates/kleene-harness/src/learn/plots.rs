//! The write-up's plots as standalone SVG files drawn from the store: the
//! learning curve, accuracy against spend (cost parity), calls against task
//! difficulty, the planner's estimated calls against actuals, and the size
//! of the plan space against the number of relations joined. No plotting
//! library: a few hundred lines of SVG is enough for line and scatter
//! charts, and the files render anywhere.

use super::{float_at, int_at, text_at, Learn};
use crate::HarnessError;

/// One series of a chart.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    /// Legend label.
    pub label: String,
    /// `(x, y)` points in drawing order.
    pub points: Vec<(f64, f64)>,
}

/// A chart of one or more series.
#[derive(Debug, Clone, PartialEq)]
pub struct Chart {
    /// Title.
    pub title: String,
    /// Axis labels.
    pub x_label: String,
    /// Axis labels.
    pub y_label: String,
    /// Series.
    pub series: Vec<Series>,
    /// Draw points only (a scatter) rather than connected lines.
    pub scatter: bool,
    /// Draw the `y = x` diagonal (estimate against actual).
    pub diagonal: bool,
    /// Log-scale the y axis (plan-space size).
    pub log_y: bool,
    /// Start the x axis at zero even when every point is to the right of it
    /// (spend, so the cheapest point is read against nothing spent).
    pub x_from_zero: bool,
    /// Draw a dashed line through the Pareto frontier of every point: the
    /// points no other point beats on both axes (less x, more y).
    pub frontier: bool,
    /// Text drawn next to a point, as `(x, y, text)`.
    pub annotations: Vec<(f64, f64, String)>,
    /// The y axis is a rate: it runs from 0 to 1 whatever the points, with
    /// the ticks written as percentages.
    pub percent_y: bool,
}

impl Chart {
    /// An empty line chart with these labels.
    pub fn new(title: &str, x_label: &str, y_label: &str) -> Self {
        Self {
            title: title.into(),
            x_label: x_label.into(),
            y_label: y_label.into(),
            series: vec![],
            scatter: false,
            diagonal: false,
            log_y: false,
            x_from_zero: false,
            frontier: false,
            annotations: vec![],
            percent_y: false,
        }
    }
}

/// The Pareto frontier of `points`, in increasing x: the points no other
/// point beats on both axes, where less x and more y is better. Ties on x
/// keep the higher y; ties on y keep the lower x.
pub fn pareto_frontier(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut sorted: Vec<(f64, f64)> = points.to_vec();
    sorted.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut out: Vec<(f64, f64)> = vec![];
    for p in sorted {
        if out.last().is_none_or(|last| p.1 > last.1) {
            out.push(p);
        }
    }
    out
}

const PALETTE: [&str; 8] = [
    "#1f77b4", "#d62728", "#2ca02c", "#ff7f0e", "#9467bd", "#8c564b", "#17becf", "#7f7f7f",
];

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn fmt_tick(v: f64) -> String {
    if v.abs() >= 1000.0 {
        format!("{:.0}", v)
    } else if v.fract() == 0.0 {
        format!("{v:.0}")
    } else if v.abs() >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// Render a chart as an SVG document (720×420, light background, legend at
/// the top right). An empty chart renders a note saying so rather than
/// nothing, so a missing plot is visible.
pub fn render(chart: &Chart) -> String {
    let (w, h) = (720.0, 420.0);
    let (left, right, top, bottom) = (64.0, 24.0, 40.0, 52.0);
    let (pw, ph) = (w - left - right, h - top - bottom);
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\" font-family=\"system-ui, sans-serif\" font-size=\"12\">\n<rect width=\"{w}\" height=\"{h}\" fill=\"#ffffff\"/>\n<text x=\"{}\" y=\"22\" text-anchor=\"middle\" font-size=\"15\" font-weight=\"600\">{}</text>\n",
        w / 2.0,
        esc(&chart.title)
    );
    let ty = |y: f64| if chart.log_y { y.max(1.0).log10() } else { y };
    let points: Vec<(f64, f64)> = chart
        .series
        .iter()
        .flat_map(|s| s.points.iter().map(|&(x, y)| (x, ty(y))))
        .collect();
    if points.is_empty() {
        out.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#888\">no data yet</text>\n</svg>\n",
            w / 2.0,
            h / 2.0
        ));
        return out;
    }
    let (mut x0, mut x1) = points
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (mut y0, mut y1) = points
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    if chart.diagonal {
        let m = x1.max(y1);
        x0 = x0.min(0.0);
        y0 = y0.min(0.0);
        x1 = m;
        y1 = m;
    }
    if !chart.log_y {
        y0 = y0.min(0.0);
    }
    if chart.x_from_zero {
        // Room on the right for the label of the most expensive point.
        x0 = x0.min(0.0);
        x1 += (x1 - x0) * 0.12;
    }
    if chart.percent_y {
        y0 = 0.0;
        y1 = 1.0;
    }
    if (x1 - x0).abs() < 1e-12 {
        x1 = x0 + 1.0;
    }
    if (y1 - y0).abs() < 1e-12 {
        y1 = y0 + 1.0;
    }
    let sx = |x: f64| left + (x - x0) / (x1 - x0) * pw;
    let sy = |y: f64| top + ph - (y - y0) / (y1 - y0) * ph;
    // Ticks and grid lines first, then the axes over them, so the grid
    // line at zero does not hide the x axis.
    for i in 0..=4 {
        let fx = x0 + (x1 - x0) * i as f64 / 4.0;
        let fy = y0 + (y1 - y0) * i as f64 / 4.0;
        let label_y = if chart.log_y { 10f64.powf(fy) } else { fy };
        let y_text = if chart.percent_y {
            format!("{:.0}%", label_y * 100.0)
        } else {
            fmt_tick(label_y)
        };
        out.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#444\">{}</text>\n<text x=\"{}\" y=\"{}\" text-anchor=\"end\" fill=\"#444\">{}</text>\n<line x1=\"{left}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#eee\"/>\n",
            sx(fx),
            top + ph + 16.0,
            fmt_tick(fx),
            left - 6.0,
            sy(fy) + 4.0,
            y_text,
            sy(fy),
            left + pw,
            sy(fy)
        ));
    }
    out.push_str(&format!(
        "<line x1=\"{left}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#444\"/>\n<line x1=\"{left}\" y1=\"{top}\" x2=\"{left}\" y2=\"{}\" stroke=\"#444\"/>\n",
        top + ph,
        left + pw,
        top + ph,
        top + ph
    ));
    out.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#222\">{}</text>\n<text transform=\"translate(14 {}) rotate(-90)\" text-anchor=\"middle\" fill=\"#222\">{}</text>\n",
        left + pw / 2.0,
        h - 12.0,
        esc(&chart.x_label),
        top + ph / 2.0,
        esc(&chart.y_label)
    ));
    if chart.diagonal {
        out.push_str(&format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#bbb\" stroke-dasharray=\"4 4\"/>\n",
            sx(x0.max(y0)),
            sy(x0.max(y0)),
            sx(x1.min(y1)),
            sy(x1.min(y1))
        ));
    }
    if chart.frontier {
        let all: Vec<(f64, f64)> = chart
            .series
            .iter()
            .flat_map(|s| s.points.iter().copied())
            .collect();
        let path: Vec<String> = pareto_frontier(&all)
            .iter()
            .map(|&(x, y)| format!("{:.1},{:.1}", sx(x), sy(ty(y))))
            .collect();
        if path.len() > 1 {
            out.push_str(&format!(
                "<polyline fill=\"none\" stroke=\"#999\" stroke-width=\"1.5\" stroke-dasharray=\"5 4\" points=\"{}\"/>\n",
                path.join(" ")
            ));
        }
    }
    for (i, s) in chart.series.iter().enumerate() {
        let color = PALETTE[i % PALETTE.len()];
        let pts: Vec<(f64, f64)> = s.points.iter().map(|&(x, y)| (sx(x), sy(ty(y)))).collect();
        if !chart.scatter && pts.len() > 1 {
            let path: Vec<String> = pts.iter().map(|(x, y)| format!("{x:.1},{y:.1}")).collect();
            out.push_str(&format!(
                "<polyline fill=\"none\" stroke=\"{color}\" stroke-width=\"2\" points=\"{}\"/>\n",
                path.join(" ")
            ));
        }
        for (x, y) in &pts {
            out.push_str(&format!(
                "<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"3\" fill=\"{color}\"/>\n"
            ));
        }
        // Legend.
        let ly = top + 6.0 + 16.0 * i as f64;
        out.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"10\" height=\"10\" fill=\"{color}\"/>\n<text x=\"{}\" y=\"{}\" fill=\"#222\">{}</text>\n",
            left + pw - 200.0,
            ly,
            left + pw - 186.0,
            ly + 9.0,
            esc(&s.label)
        ));
    }
    for (x, y, text) in &chart.annotations {
        // To the right of the point, or to its left near the right edge.
        let (ax, anchor) = if sx(*x) > left + pw - 70.0 {
            (sx(*x) - 6.0, "end")
        } else {
            (sx(*x) + 6.0, "start")
        };
        out.push_str(&format!(
            "<text x=\"{ax:.1}\" y=\"{:.1}\" text-anchor=\"{anchor}\" font-size=\"11\" fill=\"#333\">{}</text>\n",
            sy(ty(*y)) - 6.0,
            esc(text)
        ));
    }
    out.push_str("</svg>\n");
    out
}

fn rolling(values: &[bool], window: usize) -> Vec<f64> {
    let w = window.max(1);
    (0..values.len())
        .map(|i| {
            let lo = i.saturating_sub(w - 1);
            let slice = &values[lo..=i];
            slice.iter().filter(|s| **s).count() as f64 / slice.len() as f64
        })
        .collect()
}

impl Learn {
    /// The plots as `(file name, SVG)` pairs, from `evals`, `attempts`,
    /// `task_ratings` and `trace_statements`. A plot with no data says so.
    pub async fn bench_plots(&self) -> Result<Vec<(String, String)>, HarnessError> {
        self.init_bench().await?;
        let store = self.store();
        // 1. Learning curve: rolling accuracy against task number, one line
        //    per run.
        let mut curve = Chart::new("Learning curve", "task number", "accuracy (rolling 5)");
        let runs = store
            .query("SELECT run, pack, mode FROM evals GROUP BY run, pack, mode ORDER BY MIN(recorded_at)")
            .await?;
        for i in 0..runs.rows.len() {
            let run = text_at(&runs, i, 0);
            let solved: Vec<bool> = self
                .bench_curve(&run)
                .await?
                .into_iter()
                .map(|(_, s)| s)
                .collect();
            curve.series.push(Series {
                label: format!("{} {}", text_at(&runs, i, 1), text_at(&runs, i, 2)),
                points: rolling(&solved, 5)
                    .into_iter()
                    .enumerate()
                    .map(|(j, a)| (j as f64 + 1.0, a))
                    .collect(),
            });
        }
        // 2. Cost parity: cumulative accuracy against cumulative spend per
        //    (pack, mode), so two modes can be read at equal dollars.
        let mut parity = Chart::new(
            "Accuracy at cost parity",
            "cumulative dollars",
            "cumulative accuracy",
        );
        let modes = store
            .query("SELECT pack, mode FROM evals GROUP BY pack, mode ORDER BY pack, mode")
            .await?;
        for i in 0..modes.rows.len() {
            let (pack, mode) = (text_at(&modes, i, 0), text_at(&modes, i, 1));
            let rows = store
                .query(&format!(
                    "SELECT solved, dollars FROM evals WHERE pack = {} AND mode = {} ORDER BY run, seq",
                    super::s(&pack),
                    super::s(&mode)
                ))
                .await?;
            let (mut spent, mut solved, mut points) = (0.0, 0.0, vec![]);
            for j in 0..rows.rows.len() {
                spent += float_at(&rows, j, 1);
                if text_at(&rows, j, 0) == "true" {
                    solved += 1.0;
                }
                points.push((spent, solved / (j as f64 + 1.0)));
            }
            parity.series.push(Series {
                label: format!("{pack} {mode}"),
                points,
            });
        }
        // 3. Calls against difficulty: every attempt against its task's
        //    current rating, solved and failed as two series.
        let mut difficulty = Chart::new("Calls against difficulty", "task rating", "calls");
        difficulty.scatter = true;
        let attempts = store
            .query(
                "SELECT COALESCE(r.rating, t.difficulty), a.calls, a.solved FROM attempts a JOIN tasks t ON t.id = a.task LEFT JOIN task_ratings r ON r.task = a.task ORDER BY a.recorded_at",
            )
            .await?;
        let mut solved_pts = vec![];
        let mut failed_pts = vec![];
        for i in 0..attempts.rows.len() {
            let p = (float_at(&attempts, i, 0), int_at(&attempts, i, 1) as f64);
            if text_at(&attempts, i, 2) == "true" {
                solved_pts.push(p);
            } else {
                failed_pts.push(p);
            }
        }
        difficulty.series.push(Series {
            label: "solved".into(),
            points: solved_pts,
        });
        difficulty.series.push(Series {
            label: "failed".into(),
            points: failed_pts,
        });
        // 4. Estimate accuracy: the planner's estimated calls against the
        //    calls the statement made, on the diagonal when exact.
        let mut estimates = Chart::new(
            "Estimated against actual calls",
            "estimated calls",
            "actual calls",
        );
        estimates.scatter = true;
        estimates.diagonal = true;
        let stmts = store
            .query(
                "SELECT estimate, calls FROM trace_statements WHERE estimate IS NOT NULL AND calls IS NOT NULL AND error IS NULL ORDER BY started_at",
            )
            .await?;
        let mut est_pts = vec![];
        let mut space_pts = vec![];
        for i in 0..stmts.rows.len() {
            let Ok(est) = serde_json::from_str::<serde_json::Value>(&text_at(&stmts, i, 0)) else {
                continue;
            };
            if let Some(c) = est.get("calls").and_then(|v| v.as_f64()) {
                est_pts.push((c, int_at(&stmts, i, 1) as f64));
            }
            if let (Some(r), Some(o)) = (
                est.get("relations").and_then(|v| v.as_f64()),
                est.get("join_orders").and_then(|v| v.as_f64()),
            ) {
                space_pts.push((r, o));
            }
        }
        estimates.series.push(Series {
            label: "statements".into(),
            points: est_pts,
        });
        // 5. Plan space: join orders (log) against relations joined.
        let mut space = Chart::new(
            "Plan space against query shape",
            "relations joined",
            "join orders (log scale)",
        );
        space.scatter = true;
        space.log_y = true;
        space.series.push(Series {
            label: "statements".into(),
            points: space_pts,
        });
        Ok(vec![
            ("learning_curve.svg".to_string(), render(&curve)),
            ("cost_parity.svg".to_string(), render(&parity)),
            ("calls_vs_difficulty.svg".to_string(), render(&difficulty)),
            ("estimate_accuracy.svg".to_string(), render(&estimates)),
            ("plan_space.svg".to_string(), render(&space)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charts_render_lines_scatters_and_empty_notes() {
        let mut c = Chart::new("t", "x", "y");
        assert!(render(&c).contains("no data yet"));
        c.series.push(Series {
            label: "a & b".into(),
            points: vec![(1.0, 0.5), (2.0, 1.0)],
        });
        let svg = render(&c);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("<polyline"));
        assert!(svg.contains("a &amp; b"));
        c.scatter = true;
        c.diagonal = true;
        c.log_y = true;
        let svg = render(&c);
        assert!(!svg.contains("<polyline"));
        assert!(svg.contains("stroke-dasharray"));
        assert_eq!(
            rolling(&[true, false, true, true], 2),
            vec![1.0, 0.5, 0.5, 1.0]
        );
    }

    #[test]
    fn frontier_keeps_the_undominated_points_and_renders_dashed() {
        let pts = [(1.0, 0.5), (2.0, 0.4), (3.0, 0.9), (0.5, 0.2), (3.0, 0.8)];
        assert_eq!(
            pareto_frontier(&pts),
            vec![(0.5, 0.2), (1.0, 0.5), (3.0, 0.9)]
        );
        let mut c = Chart::new("t", "x", "y");
        c.scatter = true;
        c.frontier = true;
        c.x_from_zero = true;
        c.series.push(Series {
            label: "m".into(),
            points: pts.to_vec(),
        });
        c.annotations.push((1.0, 0.5, "a < b".into()));
        c.percent_y = true;
        let svg = render(&c);
        assert!(svg.contains("stroke-dasharray=\"5 4\""), "{svg}");
        assert!(svg.contains("a &lt; b"), "{svg}");
        assert!(svg.contains(">100%<") && svg.contains(">0%<"), "{svg}");
    }
}
