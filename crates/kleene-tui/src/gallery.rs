//! Renders representative screens into HTML for docs and review. Ignored by
//! default; run with
//! `KLEENE_GALLERY_OUT=dir cargo test --workspace --all-features -- gallery --ignored`.

use crate::setup::{SetupApp, Step};
use crate::theme::{Flavor, Theme};
use crate::{ui, App, Entry};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use kleene_core::{BudgetUsage, CallId, RunId, SessionId, StatementId};
use kleene_daemon::{Cursor, ServerMessage, StatementOutput};
use kleene_llm::ProviderSettings;
use kleene_trace::{TraceEvent, Traced};
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use std::time::Duration;

fn css(c: Color, fallback: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("rgb({r},{g},{b})"),
        _ => fallback.to_string(),
    }
}

/// The buffer as HTML: one `<pre>`, runs of equal style as spans.
fn to_html(backend: &TestBackend, theme: &Theme) -> String {
    let buf = backend.buffer();
    let bg = css(theme.bg, "#111");
    let fg = css(theme.fg, "#ddd");
    let mut out = format!("<pre class=\"screen\" style=\"background:{bg};color:{fg}\">");
    for y in 0..buf.area.height {
        let mut last: Option<String> = None;
        let mut run = String::new();
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            let cfg = css(cell.fg, &fg);
            let cbg = css(cell.bg, &bg);
            let mut style = format!("color:{cfg};background:{cbg};");
            let m = cell.modifier;
            if m.contains(Modifier::BOLD) {
                style.push_str("font-weight:bold;");
            }
            if m.contains(Modifier::DIM) {
                style.push_str("opacity:.6;");
            }
            if m.contains(Modifier::ITALIC) {
                style.push_str("font-style:italic;");
            }
            if m.contains(Modifier::UNDERLINED) {
                style.push_str("text-decoration:underline;");
            }
            let sym = cell
                .symbol()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            if last.as_deref() != Some(style.as_str()) {
                if let Some(prev) = last.take() {
                    out.push_str(&format!("<span style=\"{prev}\">{run}</span>"));
                    run = String::new();
                }
                last = Some(style);
            }
            run.push_str(&sym);
        }
        if let Some(s) = last {
            out.push_str(&format!("<span style=\"{s}\">{run}</span>"));
        }
        out.push('\n');
    }
    out.push_str("</pre>");
    out
}

fn ev(seq: u64, event: TraceEvent) -> ServerMessage {
    ServerMessage::Event {
        cursor: Cursor {
            generation: 1,
            sequence: seq,
        },
        traced: Traced {
            at: std::time::SystemTime::now(),
            event,
        },
    }
}

fn usage(calls: u64, tokens: u64, dollars: f64) -> BudgetUsage {
    BudgetUsage {
        calls,
        tokens,
        dollars,
        ..BudgetUsage::default()
    }
}

fn out(text: &str) -> StatementOutput {
    StatementOutput {
        text: text.into(),
        is_error: false,
        is_final: false,
    }
}

/// A run in progress: two turns done, a child session finished, and the
/// root's third turn streaming.
fn app_mid_run(width: usize) -> App {
    let mut app = App {
        width,
        ..App::default()
    };
    let run = RunId::new();
    let root = SessionId::new();
    let child = SessionId::new();
    let task = "Which project consumed the most hours in total?";
    app.entries.push(Entry::Input(task.into()));
    let mut seq = 0;
    let mut push = |app: &mut App, e: TraceEvent| {
        app.apply(ev(seq, e));
        seq += 1;
    };
    push(
        &mut app,
        TraceEvent::RunStarted {
            run,
            task: task.into(),
        },
    );
    push(
        &mut app,
        TraceEvent::SessionStarted {
            run,
            session: root,
            parent: None,
            depth: 0,
            role: "root".into(),
            task: task.into(),
        },
    );
    // Turn 1.
    let t1 = StatementId::new();
    push(
        &mut app,
        TraceEvent::StatementStarted {
            session: root,
            statement: t1,
            sql: "-- turn 1".into(),
        },
    );
    let c1 = CallId::new();
    push(
        &mut app,
        TraceEvent::CallStarted {
            statement: t1,
            call: c1,
            alias: "root".into(),
            model: "claude-opus-5".into(),
            fingerprint: "a".into(),
        },
    );
    push(
        &mut app,
        TraceEvent::CallFinished {
            call: c1,
            input_tokens: 3100,
            output_tokens: 360,
            cache_read_tokens: 2800,
            cost_usd: 0.023,
            elapsed: Duration::from_millis(2400),
            memo_hit: false,
            error: None,
        },
    );
    push(
        &mut app,
        TraceEvent::StatementFinished {
            statement: t1,
            rows: 0,
            usage: usage(1, 3460, 0.023),
            error: None,
            elapsed: Duration::from_millis(2400),
        },
    );
    let s1 = StatementId::new();
    push(
        &mut app,
        TraceEvent::StatementStarted {
            session: root,
            statement: s1,
            sql: "SELECT COUNT(*) AS rows, MIN(ordinal) AS lo, MAX(ordinal) AS hi FROM ctx".into(),
        },
    );
    push(
        &mut app,
        TraceEvent::StatementPlanned {
            statement: s1,
            explain: "γ count, min, max  ~1 row · 0 calls\n  scan ctx  60 rows\nfragment: CQ"
                .into(),
            estimate: serde_json::json!({}),
        },
    );
    push(
        &mut app,
        TraceEvent::StatementFinished {
            statement: s1,
            rows: 1,
            usage: usage(0, 0, 0.0),
            error: None,
            elapsed: Duration::from_millis(3),
        },
    );
    app.apply(ServerMessage::TurnFinished {
        session: root,
        turn: 1,
        reply: "Let me look at the shape of the context first.\n\n```sql\nSELECT COUNT(*) AS rows, MIN(ordinal) AS lo, MAX(ordinal) AS hi FROM ctx;\n```".into(),
        sql: Some("SELECT COUNT(*) AS rows, MIN(ordinal) AS lo, MAX(ordinal) AS hi FROM ctx".into()),
        results: vec![out("rows | lo | hi\n-----+----+---\n60   | 0  | 59\n1 row")],
        plan: vec![],
    });
    // Turn 2.
    let t2 = StatementId::new();
    push(
        &mut app,
        TraceEvent::StatementStarted {
            session: root,
            statement: t2,
            sql: "-- turn 2".into(),
        },
    );
    let c2 = CallId::new();
    push(
        &mut app,
        TraceEvent::CallStarted {
            statement: t2,
            call: c2,
            alias: "root".into(),
            model: "claude-opus-5".into(),
            fingerprint: "b".into(),
        },
    );
    push(
        &mut app,
        TraceEvent::CallFinished {
            call: c2,
            input_tokens: 4200,
            output_tokens: 410,
            cache_read_tokens: 3900,
            cost_usd: 0.006,
            elapsed: Duration::from_millis(1900),
            memo_hit: false,
            error: None,
        },
    );
    push(
        &mut app,
        TraceEvent::StatementFinished {
            statement: t2,
            rows: 0,
            usage: usage(1, 4610, 0.006),
            error: None,
            elapsed: Duration::from_millis(1900),
        },
    );
    let s2 = StatementId::new();
    let sql2 = "CREATE TABLE hours AS\nSELECT ordinal, llm_json('Extract project and hours from: ' || text, '{\"project\":\"string\",\"hours\":\"number\"}') AS h\nFROM ctx WHERE mentions_hours(text)";
    push(
        &mut app,
        TraceEvent::StatementStarted {
            session: root,
            statement: s2,
            sql: sql2.into(),
        },
    );
    push(&mut app, TraceEvent::StatementPlanned { statement: s2, explain: "π ordinal, λ llm_json(worker)  ~30 rows · ~30 calls · ~$0.09\n  σ λ mentions_hours(proxy)  sel 0.5 (sampled, 12 seen)\n    scan ctx  60 rows\nrules: cheap-first, cascade\nalternatives:\n  ▶ chosen: proxy then oracle  ~66 calls · ~$0.11\n    as written: oracle on every row  ~120 calls · ~$0.32".into(), estimate: serde_json::json!({}) });
    for i in 0..6 {
        let c = CallId::new();
        push(
            &mut app,
            TraceEvent::CallStarted {
                statement: s2,
                call: c,
                alias: if i % 2 == 0 {
                    "proxy".into()
                } else {
                    "worker".into()
                },
                model: String::new(),
                fingerprint: format!("c{i}"),
            },
        );
        push(
            &mut app,
            TraceEvent::CallFinished {
                call: c,
                input_tokens: 220,
                output_tokens: 40,
                cache_read_tokens: 0,
                cost_usd: if i % 3 == 0 { 0.0 } else { 0.0011 },
                elapsed: Duration::from_millis(400),
                memo_hit: i % 3 == 0,
                error: None,
            },
        );
    }
    push(
        &mut app,
        TraceEvent::StatementFinished {
            statement: s2,
            rows: 28,
            usage: usage(66, 15200, 0.104),
            error: None,
            elapsed: Duration::from_millis(7800),
        },
    );
    push(
        &mut app,
        TraceEvent::SessionStarted {
            run,
            session: child,
            parent: Some(root),
            depth: 1,
            role: "worker".into(),
            task: "Resolve which project 'the migration' refers to in notes 14, 22 and 41".into(),
        },
    );
    let cs = StatementId::new();
    push(
        &mut app,
        TraceEvent::StatementStarted {
            session: child,
            statement: cs,
            sql: "SELECT ordinal, text FROM ctx WHERE ordinal IN (14, 22, 41)".into(),
        },
    );
    push(
        &mut app,
        TraceEvent::StatementFinished {
            statement: cs,
            rows: 3,
            usage: usage(0, 0, 0.0),
            error: None,
            elapsed: Duration::from_millis(2),
        },
    );
    app.apply(ServerMessage::TurnFinished {
        session: child,
        turn: 1,
        reply: "```sql\nSELECT ordinal, text FROM ctx WHERE ordinal IN (14, 22, 41);\nFINAL FROM (SELECT 'Osprey' AS project);\n```".into(),
        sql: Some("SELECT ordinal, text FROM ctx WHERE ordinal IN (14, 22, 41);\nFINAL FROM (SELECT 'Osprey' AS project)".into()),
        results: vec![out("ordinal | text\n--------+------\n14      | Meeting notes 2026-03-15. Project Osprey: the migration…\n3 rows"), out("FINAL\nproject\n-------\nOsprey\n1 row")],
        plan: vec![],
    });
    push(
        &mut app,
        TraceEvent::SessionFinished {
            session: child,
            outcome: "final".into(),
            turns: 1,
            usage: usage(2, 3100, 0.004),
        },
    );
    app.apply(ServerMessage::TurnFinished {
        session: root,
        turn: 2,
        reply: "Only some notes mention hours. A cheap proxy filters those, then a worker extracts project and hours as JSON.\n\n```sql\nCREATE TABLE hours AS\nSELECT ordinal, llm_json('Extract project and hours from: ' || text, '{\"project\":\"string\",\"hours\":\"number\"}') AS h\nFROM ctx WHERE mentions_hours(text);\n```".into(),
        sql: Some(sql2.into()),
        results: vec![out("28 rows")],
        plan: vec![
            ("peek at the context".into(), "done".into()),
            ("extract hours per note".into(), "doing".into()),
            ("sum per project".into(), "todo".into()),
        ],
    });
    // Turn 3: streaming right now.
    let t3 = StatementId::new();
    push(
        &mut app,
        TraceEvent::StatementStarted {
            session: root,
            statement: t3,
            sql: "-- turn 3".into(),
        },
    );
    let c3 = CallId::new();
    push(
        &mut app,
        TraceEvent::CallStarted {
            statement: t3,
            call: c3,
            alias: "root".into(),
            model: "claude-opus-5".into(),
            fingerprint: "d".into(),
        },
    );
    app.apply(ServerMessage::CallDelta {
        call: c3,
        text: "The hours table has 28 rows across six projects. Summing per project and picking the top one:\n\n```sql\nFINAL FROM (\n  SELECT h.project, SUM(h.hours) AS total_hours\n  FROM hours\n  GROUP BY h.project\n  ORDER BY total_hours DESC\n  LIMIT 1\n);".into(),
    });
    app
}

fn render(app: &App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    to_html(terminal.backend(), &app.theme)
}

fn render_setup(app: &SetupApp, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| crate::setup::draw(f, app)).unwrap();
    to_html(terminal.backend(), &app.theme)
}

fn key(app: &mut SetupApp, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

#[test]
#[ignore = "writes HTML screenshots; set KLEENE_GALLERY_OUT"]
fn gallery() {
    let Some(dir) = std::env::var_os("KLEENE_GALLERY_OUT") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (120u16, 40u16);
    let mut shots: Vec<(&str, String)> = Vec::new();

    let mut s = SetupApp::new(
        ProviderSettings::default(),
        "/Users/marcus/.config/kleene/config.toml".into(),
    );
    key(&mut s, KeyCode::Char(' '));
    for _ in 0..3 {
        key(&mut s, KeyCode::Down);
    }
    key(&mut s, KeyCode::Char(' '));
    key(&mut s, KeyCode::Up);
    key(&mut s, KeyCode::Up);
    key(&mut s, KeyCode::Up);
    shots.push(("setup-choose", render_setup(&s, w, h)));
    key(&mut s, KeyCode::Enter);
    for c in "sk-ant-api03-3QkZv7wLx0mN1pR8sT2uV4wX6yZ8".chars() {
        key(&mut s, KeyCode::Char(c));
    }
    shots.push(("setup-credentials", render_setup(&s, w, h)));
    key(&mut s, KeyCode::Enter);
    key(&mut s, KeyCode::Enter);
    assert_eq!(s.step, Step::Fields(1));
    key(&mut s, KeyCode::Enter);
    for c in "BSA3qLm9xP2vR7tW1yZ5cE8gH0jK4nQ6".chars() {
        key(&mut s, KeyCode::Char(c));
    }
    shots.push(("setup-web-search", render_setup(&s, w, h)));
    key(&mut s, KeyCode::Enter);
    assert_eq!(s.step, Step::Review);
    shots.push(("setup-review", render_setup(&s, w, h)));

    let mut empty = App {
        workspace: "/Users/marcus/notes".into(),
        input: "Which project consumed the most hours in total?".into(),
        width: w as usize - 2,
        ..App::default()
    };
    shots.push(("welcome", render(&empty, w, h)));
    empty.input = "/tr".into();
    shots.push(("completion", render(&empty, w, h)));
    empty.input.clear();
    empty.theme = Theme::light();
    shots.push(("welcome-latte", render(&empty, w, h)));

    let mut app = app_mid_run(w as usize - 2);
    shots.push(("run", render(&app, w, h)));
    app.show_plans = true;
    app.scroll = 14;
    shots.push(("run-plans", render(&app, w, h)));
    app.show_plans = false;
    app.scroll = 0;
    app.entries.push(Entry::Input(
        "/trace SELECT depth, role, outcome, turns, calls, dollars FROM trace_sessions".into(),
    ));
    app.apply(ServerMessage::Table {
        columns: vec![
            "depth".into(),
            "role".into(),
            "outcome".into(),
            "turns".into(),
            "calls".into(),
            "dollars".into(),
        ],
        rows: vec![
            vec![
                "0".into(),
                "root".into(),
                "NULL".into(),
                "3".into(),
                "69".into(),
                "0.1330".into(),
            ],
            vec![
                "1".into(),
                "worker".into(),
                "final".into(),
                "1".into(),
                "2".into(),
                "0.0040".into(),
            ],
        ],
        tag: None,
    });
    shots.push(("trace", render(&app, w, h)));
    app.entries.pop();
    app.entries.pop();
    for flavor in [Flavor::Macchiato, Flavor::Frappe, Flavor::Latte] {
        app.theme = Theme::flavor(flavor);
        shots.push((flavor.name(), render(&app, w, h)));
    }
    app.theme = Theme::dark();
    app.entries.push(Entry::Help);
    shots.push(("help", render(&app, w, h)));

    for (name, html) in &shots {
        std::fs::write(dir.join(format!("{name}.html")), html).unwrap();
    }
}
