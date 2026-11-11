# gh-achiv

Short Rust CLI for GitHub profile achievements. It uses the existing GitHub CLI
authentication (`gh auth login`) and does not store a GitHub token.

## Quick start

Run commands in a checkout of the repository. The repository is detected from
the current directory; pass `--repo OWNER/REPO` to override it.

```text
gh-achiv --help
gh-achiv status
gh-achiv quickdraw -m "Describe a real issue to create and close"
gh-achiv pullshark -m "Describe the change for the PR"
gh-achiv yolo -m "Describe the change for the PR"
```

Each workflow prints what it will do and asks once for `y`. Quickdraw creates an
issue using the first line as its title, waits between writes, and closes it.
Pull Shark and YOLO use an open PR on the current branch, or create one when the
branch has a real diff from the default branch. The branch must already be
pushed. YOLO requires a PR with no reviews. `-m` is needed when the CLI must
create a PR; add `--number N` to select an open PR explicitly. Add
`--repo OWNER/REPO` outside a checkout. Pull Shark's first tier requires two
merged PRs, so one merge does not guarantee the badge.

## Available achievements

- **Quickdraw**: CLI can create and promptly close an issue.
- **Pull Shark**: CLI can merge your open pull request.
- **YOLO**: CLI can merge your open pull request after checking it has no reviews.
- **Pair Extraordinaire**: requires meaningful co-authored work with another user.
- **Galaxy Brain**: depends on accepted answers and discussion participation.
- **Starstruck**: depends on other people starring your repository.
- **Public Sponsor**: requires a genuine public sponsorship.
- **Arctic Code Vault Contributor** and **Mars 2020 Contributor**: historical
  achievements from past GitHub programs; they are no longer earnable.

The CLI does not simulate the achievements that require another person's
participation or recognition. GitHub determines eligibility and badge timing;
the achievement tab is not exposed through the GitHub CLI/API.

## Safety

- Every workflow asks once before remote changes; `y` proceeds and Enter cancels.
- Writes are serialized with at least a two-second gap; Quickdraw has a larger
  safety window and stops closing if that window is almost over.
- Rate-limit and abuse responses stop the command without automatic retries.
- Public repositories are allowed by default. Pass `--allow-private` to opt in
  to a private repository.

Requires Rust 2024 and the GitHub CLI (`gh`) authenticated on the account to use.
