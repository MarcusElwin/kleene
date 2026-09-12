# CI

`github-ci.yml` is the GitHub Actions workflow (fmt, clippy with warnings
denied, tests, docs). `release.yml` builds the `callgebra` binary for four
targets on `v*` tags, attaches tarballs and SHA-256 checksums to a GitHub
release, and prints the values `Formula/callgebra.rb` needs. Both live here
because the coding agent's GitHub App token cannot create files under
`.github/workflows/`. To enable them:

```bash
mkdir -p .github/workflows
git mv ci/github-ci.yml .github/workflows/ci.yml
git mv ci/release.yml .github/workflows/release.yml
```

Then `git tag v0.1.0 && git push --tags` publishes a release, and
`install.sh` (curl) and the Homebrew formula pick it up.

The same checks run locally with the commands in `CLAUDE.md`.
