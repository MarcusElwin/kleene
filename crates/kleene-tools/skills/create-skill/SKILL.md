---
name: create-skill
description: Write a new skill (a SKILL.md with frontmatter) that Kleene loads for every session, and check it with /skills
---

# Create a skill

A skill is a directory holding a `SKILL.md`: YAML frontmatter with `name`
and `description`, then Markdown the model reads on demand. Only the name
and description go into every prompt; the body is read with
`SELECT text FROM skill('name')` when the task calls for it, so a skill
costs nothing until it is used.

## Where skills live

| Scope | Directory |
|---|---|
| this project | `.kleene/skills/<name>/SKILL.md` (also `.agents/skills/`, `.claude/skills/`) |
| this user | `$KLEENE_CONFIG_DIR/skills/<name>/SKILL.md` (default `~/.config/kleene/skills/`), also `~/.agents/skills/` |
| built in | shipped with the binary; a project or user skill of the same name replaces it |

## Write one

```sql
CALL mkdir('.kleene/skills/release-notes');
CALL write_file('.kleene/skills/release-notes/SKILL.md', $$---
name: release-notes
description: Draft release notes from the commits since the last tag, grouped by area
---

# Release notes

1. `SELECT sha, message FROM git_log(200)` and stop at the last tag line.
2. Group messages by their leading `area:` prefix.
3. One bullet per commit, imperative mood, no ticket numbers.
$$);
```

Rules for the body:

- Under 2,000 words. Put long reference material in files beside it and
  name them; the model reads them with `read`.
- Say when the skill applies in the description, in one specific sentence;
  the description is what the model sees when deciding to load it.
- Prefer CallSQL in examples: table functions in `FROM`, side effects as
  `CALL`.

## Check it

`/skills` in the TUI (or `kleene skills`) lists every loaded skill with its
source; `/skills release-notes` prints the body. A new project skill is
picked up by the next run; the daemon reloads skills on `/setup` save or
`/mcp add`.
