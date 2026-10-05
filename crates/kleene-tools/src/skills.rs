//! Skills: `SKILL.md` files the model reads on demand.
//!
//! A skill is a directory with a `SKILL.md` whose YAML frontmatter names it
//! and says when it applies; the body is Markdown. Only the name and the
//! description go into the prompt; the body comes back through the `skill`
//! table function. Skills come from three places, later ones replacing
//! earlier ones of the same name: the ones built into the binary
//! ([`builtin`]), the user's (`<config dir>/skills`, `~/.agents/skills`)
//! and the project's (`.kleene/skills`, `.agents/skills`, `.claude/skills`
//! under the workspace).

use std::path::{Path, PathBuf};

/// One loaded skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// Name, from the frontmatter or the directory.
    pub name: String,
    /// When it applies, from the frontmatter.
    pub description: String,
    /// `builtin`, `user` or `project`.
    pub source: String,
    /// Where it was read from (`builtin` for the shipped ones).
    pub path: String,
    /// The Markdown body, frontmatter stripped.
    pub body: String,
}

/// The skills shipped with the binary.
pub fn builtin() -> Vec<Skill> {
    const SHIPPED: &[(&str, &str)] = &[
        (
            "create-skill",
            include_str!("../skills/create-skill/SKILL.md"),
        ),
        (
            "run-benchmark",
            include_str!("../skills/run-benchmark/SKILL.md"),
        ),
        ("code-task", include_str!("../skills/code-task/SKILL.md")),
        (
            "add-mcp-server",
            include_str!("../skills/add-mcp-server/SKILL.md"),
        ),
        (
            "plan-queries",
            include_str!("../skills/plan-queries/SKILL.md"),
        ),
        (
            "long-context",
            include_str!("../skills/long-context/SKILL.md"),
        ),
    ];
    SHIPPED
        .iter()
        .map(|(name, text)| parse(name, text, "builtin", "builtin"))
        .collect()
}

/// Parse a `SKILL.md`: frontmatter between `---` lines with `name:` and
/// `description:`; the rest is the body. A missing name falls back to
/// `fallback_name`.
pub fn parse(fallback_name: &str, text: &str, source: &str, path: &str) -> Skill {
    let text = text.replace("\r\n", "\n");
    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut body = text.as_str();
    if let Some(rest) = text.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                if let Some((k, v)) = line.split_once(':') {
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    match k.trim() {
                        "name" if !v.is_empty() => name = v.to_string(),
                        "description" => description = v.to_string(),
                        _ => {}
                    }
                }
            }
            body = rest[end + 4..].trim_start_matches('\n');
        }
    }
    Skill {
        name,
        description,
        source: source.to_string(),
        path: path.to_string(),
        body: body.trim().to_string(),
    }
}

/// The directories skills are read from for a workspace and a config
/// directory, in loading order (later replaces earlier).
pub fn roots(workspace: &Path, config_dir: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut out = vec![(config_dir.join("skills"), "user")];
    if let Some(home) = std::env::var_os("HOME") {
        out.push((PathBuf::from(home).join(".agents").join("skills"), "user"));
    }
    for d in [".kleene/skills", ".agents/skills", ".claude/skills"] {
        out.push((workspace.join(d), "project"));
    }
    out
}

/// Every skill visible to a session: built-in ones, then the user's, then
/// the project's, deduplicated by name with the last loaded winning.
/// Sorted by name.
pub fn discover(workspace: &Path, config_dir: &Path) -> Vec<Skill> {
    let mut all = builtin();
    for (root, source) in roots(workspace, config_dir) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            let file = dir.join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let fallback = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            all.push(parse(&fallback, &text, source, &file.to_string_lossy()));
        }
    }
    let mut out: Vec<Skill> = vec![];
    for s in all {
        out.retain(|o| o.name != s.name);
        out.push(s);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The project instructions file, if the workspace has one: the first of
/// `KLEENE.md`, `AGENTS.md`, `CLAUDE.md` at its root, cut at `max_chars`.
pub fn project_instructions(workspace: &Path, max_chars: usize) -> Option<String> {
    for name in ["KLEENE.md", "AGENTS.md", "CLAUDE.md"] {
        if let Ok(text) = std::fs::read_to_string(workspace.join(name)) {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let mut out: String = text.chars().take(max_chars).collect();
            if out.len() < text.len() {
                out.push_str("\n… (cut; read the file for the rest)");
            }
            return Some(out);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_skills_parse_with_names_and_descriptions() {
        let b = builtin();
        assert_eq!(b.len(), 6);
        for s in &b {
            assert!(!s.description.is_empty(), "{}", s.name);
            assert!(s.body.starts_with("# "), "{}: {}", s.name, &s.body[..20]);
            assert_eq!(s.source, "builtin");
        }
        assert!(b.iter().any(|s| s.name == "create-skill"));
    }

    #[test]
    fn frontmatter_is_optional_and_quotes_are_stripped() {
        let s = parse(
            "dir-name",
            "---\nname: \"x\"\ndescription: 'why'\n---\n\n# X\nbody",
            "user",
            "/p",
        );
        assert_eq!((s.name.as_str(), s.description.as_str()), ("x", "why"));
        assert_eq!(s.body, "# X\nbody");
        let s = parse("dir-name", "# plain\n", "user", "/p");
        assert_eq!(s.name, "dir-name");
        assert_eq!(s.body, "# plain");
    }

    #[test]
    fn project_skills_replace_user_and_builtin_ones() {
        let ws = tempfile::tempdir().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let user = cfg.path().join("skills").join("code-task");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(
            user.join("SKILL.md"),
            "---\nname: code-task\ndescription: mine\n---\nuser body",
        )
        .unwrap();
        let proj = ws.path().join(".kleene").join("skills").join("extra");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("SKILL.md"),
            "---\ndescription: project one\n---\nproject body",
        )
        .unwrap();
        let all = discover(ws.path(), cfg.path());
        let code = all.iter().find(|s| s.name == "code-task").unwrap();
        assert_eq!(
            (code.source.as_str(), code.body.as_str()),
            ("user", "user body")
        );
        let extra = all.iter().find(|s| s.name == "extra").unwrap();
        assert_eq!(extra.source, "project");
        assert_eq!(all.len(), 7);
        assert!(all.windows(2).all(|w| w[0].name < w[1].name));
    }

    #[test]
    fn instructions_come_from_the_first_file_found() {
        let ws = tempfile::tempdir().unwrap();
        assert_eq!(project_instructions(ws.path(), 100), None);
        std::fs::write(ws.path().join("CLAUDE.md"), "claude rules").unwrap();
        std::fs::write(ws.path().join("AGENTS.md"), "agent rules that are long").unwrap();
        assert_eq!(
            project_instructions(ws.path(), 100).as_deref(),
            Some("agent rules that are long")
        );
        let cut = project_instructions(ws.path(), 5).unwrap();
        assert!(cut.starts_with("agent\n… (cut"));
    }
}
