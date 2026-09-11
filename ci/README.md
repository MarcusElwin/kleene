# CI

`github-ci.yml` is the GitHub Actions workflow (fmt, clippy with warnings
denied, tests, docs). It lives here because the coding agent's GitHub App
token cannot create files under `.github/workflows/`. To enable it:

```bash
mkdir -p .github/workflows
git mv ci/github-ci.yml .github/workflows/ci.yml
```

The same checks run locally with the commands in `CLAUDE.md`.
