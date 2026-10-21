---
name: write-changelog
description: Update this repository's CHANGELOG.md for a new release. Use whenever the user asks to write, generate, refresh, or prepare release notes or a changelog, add a release version, summarize changes since the previous version, or prepare the changelog before a release. Inspect every commit from the previous released version through HEAD, choose the appropriate semantic version, and write concise professional Added and Fixed entries in the repository's established format.
---

# Write Changelog

Update `CHANGELOG.md` with accurate, user-facing release notes derived from the
complete committed history since the previous release.

## Establish the release range

1. Read `CHANGELOG.md` completely to learn its language, headings, ordering,
   version syntax, date format, and comparison-link conventions.
2. Inspect version sources such as Git tags and project manifests. Treat the
   newest reachable stable version tag matching the repository's release-tag
   convention as the previous released version. Ignore unrelated tags.
3. If tags and the changelog disagree, investigate the relevant commits and
   version files. Do not silently treat an untagged changelog section as a
   published release.
4. Set the analysis range from the previous release tag, exclusive, through
   `HEAD`, inclusive. The requested release notes describe committed work only;
   do not include uncommitted worktree changes unless the user explicitly asks.

Useful commands include:

```bash
git tag --merged HEAD --sort=-version:refname
git log --reverse --date=short --format='%H%x09%ad%x09%s' <previous-tag>..HEAD
git diff --stat <previous-tag>..HEAD
```

## Inspect every commit

- Review every commit in `<previous-tag>..HEAD`, including merge commits. Track
  each commit during analysis so none are missed.
- Inspect commit bodies and the actual changed files or patches when a subject
  is vague, overly technical, or potentially misleading. Do not derive release
  notes from commit subjects alone.
- Consolidate multiple commits that implement or repair the same user-visible
  behavior into one entry. Avoid duplicate entries for follow-up fixes, tests,
  refactors, formatting, dependency bookkeeping, or merge commits.
- Account for internal-only commits during analysis, but omit them from the
  published notes when they do not change user-visible behavior. Never invent a
  feature or fix that the history does not support.

## Select the version

Follow Semantic Versioning and the repository's existing release cadence:

- Bump the major version for an incompatible public change in a stable release.
- Bump the minor version for user-visible functionality that is backward
  compatible.
- Bump the patch version when the release contains fixes and maintenance only.
- For a `0.x` project, follow its demonstrated convention; absent another
  convention, use a minor bump for a breaking change.

If the evidence supports more than one version and the choice would materially
affect the release, ask the user instead of guessing. A version explicitly
provided by the user takes precedence when it is valid and consistent with the
requested release.

## Write the release notes

Insert the new release immediately below `Unreleased`, using the current local
date and the established heading format. Preserve existing releases unchanged.

- Use `Added` for new user-visible capabilities and `Fixed` for corrected
  behavior. Include only headings that have entries.
- Describe outcomes, not implementation details or commit mechanics.
- Use professional, concise, past-tense English. Keep each bullet specific and
  understandable without reading the source code.
- Follow the repository's bilingual convention: write the English sections
  first, then matching Simplified Chinese `新增` and `修复` sections in the same
  order. The Chinese text should be a natural translation with identical scope,
  not new or missing claims.
- Avoid commit hashes, issue IDs, contributor names, marketing language, and
  low-value phrases such as "various improvements" unless the repository's
  existing format requires them.

Example structure:

```markdown
## [0.2.0] - 2026-09-21

### Added

- Added ...

### Fixed

- Fixed ...

### 新增

- 新增……

### 修复

- 修复……
```

## Maintain comparison links

Update the link definitions at the end of `CHANGELOG.md`:

- Point `[Unreleased]` from the new release tag to `HEAD`.
- Add the new version link comparing the previous release tag with the new tag.
- Preserve all earlier version links and follow the existing tag-prefix style.

## Verify the result

Before finishing:

1. Reconcile the final bullets against the complete commit checklist.
2. Confirm the version and date appear once, section order matches the file,
   English and Chinese bullets correspond, and comparison links are valid.
3. Review `git diff -- CHANGELOG.md` to ensure no prior release notes or
   unrelated worktree edits were overwritten.
4. Report the selected commit range, new version, and changelog sections added.
   Do not create a tag, commit, or release unless the user separately requests
   it.
