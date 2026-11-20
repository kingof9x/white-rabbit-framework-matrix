# Claude Code notes

## Secrets in commands

Commands approved with "Yes, and don't ask again" are saved word for word in
`.claude/settings.local.json`, including any password, API key or token in them.

- Never put a password, API key or token directly in a command. Read it from an
  environment variable (e.g. `$API_TOKEN`) or a file that git ignores instead.
- If a command can't avoid containing a secret, stop and warn me before running
  it, and remind me not to choose "don't ask again" for it.
- Never commit `.claude/settings.local.json` (it is in `.gitignore`).

<!-- github-workflow:start (same in all kingof9x repos; edit the template, not here) -->
## GitHub workflow (follow this every time)

Callum tests and merges. You write the code and open the pull request (PR).
He is not a git expert, so keep his side to: open the PR, test the build, merge.

1. **Branch.** Work on a branch named `claude/<short-topic>`. Never commit or push to `main`,
   never force-push, never merge a PR, never delete branches or tags.
2. **One PR per task, opened early.** Push the branch and open the PR straight away, before the
   build finishes. Fill in the PR template (What changed / How to test / What you should see /
   Notes and risks) in plain language. If `gh pr create` fails, use the REST API
   (`gh api repos/kingof9x/white-rabbit-framework-matrix/pulls -X POST ...`). If you can't open it, give this exact link:
   `https://github.com/kingof9x/white-rabbit-framework-matrix/compare/main...BRANCH?expand=1`
3. **Keep the branch up to date by merging.** If `main` moved on, merge `main` into your branch.
   Don't rebase or force-push a branch that is already pushed.
4. **Wait for the test build.** Every PR is built automatically. The top of the PR description
   shows ⏳ Building, then ✅ with direct download links (also posted as a 🧪 Test builds comment).
   Don't tell Callum to test until it shows ✅. If the build fails, fix it before handing off.
5. **Hand off the same way every time.** End with:
   - the PR link
   - the Actions link for the branch: `https://github.com/kingof9x/white-rabbit-framework-matrix/actions?query=branch%3ABRANCH`
   - the direct download link(s) from the PR (or say it's still building, with the Actions link)
   - 2–5 numbered test steps, ending with what he should see if it works
6. **Releases are Callum's.** He publishes them from Actions → Build and release → Run workflow
   (version box: `next` = last release + 1, a number, or empty for a test build only).
   Don't create releases or tags yourself, and don't bump version numbers for a release.
7. **Shared files.** The Build and release workflow, `.github/workflows/pr-test-builds.yml` and
   `.github/pull_request_template.md` are the same across all his repos. Don't change them unless
   he asks; say so if a task seems to need it.

If anything here can't be done (no push access, the PR can't be opened, the build is red),
say so plainly at once rather than at the end.
<!-- github-workflow:end -->

<!-- rust-fast-loop:start (same in all kingof9x Rust repos) -->
## Rust: fast build loop in agent sessions

The Rust code lives in `rust`. Keep compiles short while you work:

- Check your work with `cargo check` and `cargo test` (debug builds). Use `cargo clippy` if the CI runs it.
- **Never build with `--release` (or `--profile ci`) in a session.** Release builds are slow on purpose and
  only happen in CI: every PR gets a test build, and Callum makes real releases with Run workflow.
- Build only the crate you changed (`-p <crate>`) when the workspace has several.
- Dev builds use reduced debug info (`[profile.dev] debug = "line-tables-only"` in `Cargo.toml`) to
  compile and link faster. Don't turn it back up unless a debugger session needs it.
- Don't add new dependencies casually: each one adds compile time in every session and CI run.
<!-- rust-fast-loop:end -->
