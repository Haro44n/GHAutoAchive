# gh-achievements

A confirmation-first Rust CLI for a few GitHub profile achievements. It uses the
user's existing `gh` authentication and never stores a GitHub token.

## Requirements

- Rust toolchain (edition 2024)
- GitHub CLI (`gh`) installed and authenticated with `gh auth login`
- For `pr`, a local checkout with both base and head refs available

## Commands

```text
gh-achievements status
gh-achievements quickdraw --repo OWNER/REPO --title "A useful issue" --body "Describe a real task"
gh-achievements pr --repo OWNER/REPO --base main --head feature/real-change --title "Explain the change" --body "Why this change helps"
gh-achievements merge --repo OWNER/REPO --number 123
```

Repositories must be public by default. Add `--allow-private` to explicitly
permit a private repository. Each remote write asks for confirmation. Creating
an issue and closing it are separate confirmations; the issue is left open if
the second confirmation is declined. PR creation prints the local diff first
and refuses an empty diff. Merge only proceeds for the authenticated user's
open PR with no submitted reviews and requires typing `MERGE`.

## Safety limits

- Mutating requests run serially, with at least two seconds between requests.
  The last request time is stored under `$XDG_STATE_HOME/gh-achievements` or
  `$HOME/.local/state/gh-achievements`; there is no daily action quota.
- A lock prevents concurrent CLI instances. Quickdraw observes the same
  two-second gap between issue creation and closing, and leaves the issue open
  if the five-minute qualification window is nearly over.
- On a rate-limit or abuse response, the command stops without retrying. Wait
  at least the time requested by GitHub before running another action. The
  two-second gap is a pacing guard, not a guarantee against account-specific
  or endpoint-specific limits.

GitHub determines which actions qualify and when achievements appear. The
achievement tab is not exposed through the GitHub CLI/API, so `status` reports
the authenticated account and links to the profile rather than claiming a
verified badge count. Some achievement criteria are inferred from GitHub's
achievement hints and may change.
