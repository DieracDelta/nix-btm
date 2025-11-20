# Tooling rules for Opencode
- Use Opencode tools only: read, write, edit, list, glob, grep, webfetch, bash, task, todowrite, todoread.
- Do NOT call non-existent tools like Repo_browser.*.
- Prefer `edit` for modifying existing files; use `read` to inspect before editing.
- IGNORE node_modules, target, .git, target_dir, .opencode
