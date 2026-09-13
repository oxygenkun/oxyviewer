---
name: git-commit
description: Plan, split, draft, review, or create Git commits for this repository. Apply when preparing commits, proposing commit messages, or checking whether commits follow the repository convention.
---

# Git Commit Messages

Follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/).

## Commit Grouping

Before staging, inspect the complete status and diff, including untracked files,
and decide automatically whether the work forms one coherent commit or several.

- Split changes when they represent independently reviewable or revertible
  concerns, use materially different commit types or scopes, or include unrelated
  pre-existing worktree edits.
- Keep implementation together with its focused tests and required documentation.
  Keep file moves together with the link or import updates required by the move.
- Stage each selected group explicitly, inspect its staged diff, and write a
  message that describes only that group. Do not use broad staging that could
  absorb unrelated changes.
- Do not ask the user to choose when the grouping is clear. Ask only when
  overlapping edits or unclear intent make a safe split materially ambiguous.
- After each commit, verify the commit and remaining worktree state before
  continuing with another group.

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
- Keep each commit focused on one coherent change; apply the grouping rules
  above instead of assuming the entire worktree belongs in one commit.

Before committing, inspect the actual diff so the message describes only the changes included in that commit.
