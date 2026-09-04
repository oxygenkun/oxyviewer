---
name: git-commit
description: Draft, review, or use Git commit messages for this repository. Apply when preparing a commit, proposing a commit message, or checking whether a commit message follows the repository convention.
---

# Git Commit Messages

Follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/).

Use this structure:

```text
<type>[optional scope]: <one-sentence summary>

- <optional implementation detail>
- <optional implementation detail>

[optional footer(s)]
```

## Requirements

- Start with a concise, specific, imperative summary in the form `<type>[optional scope]: <description>`.
- Use `feat` for new behavior and `fix` for bug fixes. Other common types are `docs`, `refactor`, `perf`, `test`, `build`, `ci`, and `chore`.
- Add a lowercase scope when it clarifies the affected area, for example `feat(library): add persistent folder indexing`.
- For a non-trivial change that the summary cannot fully explain, leave one blank line and add a Markdown bullet list of the important implementation details.
- Omit the body for simple commits, and do not repeat the summary in the body.
- Mark breaking changes with `!` before the colon, a `BREAKING CHANGE: <description>` footer, or both.
- Keep each commit focused on one coherent change.

Before committing, inspect the actual diff so the message describes only the changes included in that commit.
