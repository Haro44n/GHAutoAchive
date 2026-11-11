use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Output};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MIN_MUTATION_INTERVAL: Duration = Duration::from_secs(2);
const QUICKDRAW_PAUSE: Duration = MIN_MUTATION_INTERVAL;
const QUICKDRAW_CUTOFF: Duration = Duration::from_secs(4 * 60);

#[derive(Debug)]
struct CliError(String);

type Result<T> = std::result::Result<T, CliError>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {}", error.0);
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        print_help();
        return Ok(());
    };
    let rest: Vec<String> = args.collect();
    match command.to_ascii_lowercase().as_str() {
        "quickdraw" | "quick-draw" => achievement_quickdraw(&rest),
        "pullshark" | "pull-shark" => achievement_merge(&rest, false),
        "yolo" => achievement_merge(&rest, true),
        "status" => status(&rest),
        "pr" => create_pr(&rest),
        "merge" => merge_pr(&rest),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(CliError(format!(
            "unknown achievement or command `{other}`\n\n{HELP}"
        ))),
    }
}

const HELP: &str = "gh-achiv — GitHub achievement helper

Usage:
  gh-achiv --help
  gh-achiv status
  gh-achiv quickdraw -m \"Issue title and description\" [--repo OWNER/REPO]
  gh-achiv pullshark [-m \"PR description\"] [--repo OWNER/REPO] [--number N]
  gh-achiv yolo [-m \"PR description\"] [--repo OWNER/REPO] [--number N]
  gh-achiv pr --repo OWNER/REPO --base BRANCH --head BRANCH --title TITLE --body TEXT
  gh-achiv merge --repo OWNER/REPO --number N

Achievements:
  Quickdraw   Creates an issue from -m, then closes it after one confirmation.
  Pull Shark  Merges your open PR; the first tier requires two merged PRs.
  YOLO        Same flow, and checks that the PR has no reviews.
  Pair Extraordinaire  Requires meaningful co-authored commits with another person.
  Galaxy Brain         Requires accepted answers/discussion participation.
  Starstruck           Depends on other users starring your repository.
  Public Sponsor       Requires sponsorship of an eligible open-source maintainer.
  Arctic Code Vault Contributor, Mars 2020 Contributor  Historical; no longer earnable.

For Pull Shark/YOLO, run inside a GitHub checkout with the current branch pushed.
The repository is detected automatically; --repo overrides detection. -m is the
issue/PR text. Each achievement workflow asks once; answer y to proceed. Actions
are serialized, rate limits are respected, and GitHub decides badge eligibility.";

fn print_help() {
    println!("{HELP}");
}

#[derive(Default)]
struct Options {
    repo: Option<String>,
    title: Option<String>,
    body: Option<String>,
    base: Option<String>,
    head: Option<String>,
    number: Option<String>,
    allow_private: bool,
    message: Option<String>,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self> {
        let mut options = Self::default();
        let mut i = 0;
        while i < args.len() {
            let key = args[i].as_str();
            if key == "--allow-private" {
                options.allow_private = true;
                i += 1;
                continue;
            }
            if !key.starts_with('-') {
                return Err(CliError(format!("unexpected argument `{key}`")));
            }
            let value = args
                .get(i + 1)
                .ok_or_else(|| CliError(format!("missing value for {key}")))?
                .clone();
            match key {
                "--repo" | "-r" => options.repo = Some(value),
                "--title" | "-t" => options.title = Some(value),
                "--body" => options.body = Some(value.clone()),
                "--message" | "-m" => {
                    options.message = Some(value.clone());
                    options.body = Some(value);
                }
                "--base" => options.base = Some(value),
                "--head" => options.head = Some(value),
                "--number" => options.number = Some(value),
                _ => return Err(CliError(format!("unknown option `{key}`"))),
            }
            i += 2;
        }
        Ok(options)
    }

    fn require(value: &Option<String>, name: &str) -> Result<String> {
        value
            .clone()
            .ok_or_else(|| CliError(format!("missing required option {name}")))
    }
}

fn status(args: &[String]) -> Result<()> {
    if !args.is_empty() {
        return Err(CliError("status takes no options".into()));
    }
    let login = gh_text(&["api", "user", "--jq", ".login"])?;
    println!("Authenticated to GitHub as {login} (via gh).\n");
    println!("Achievement progress cannot currently be read through the GitHub CLI/API.");
    println!("Check your profile: https://github.com/{login}?tab=achievements");
    println!("\nSupported actions:");
    println!("  Quickdraw   Close an issue or PR shortly after opening it.");
    println!("  Pull Shark  Have your PR merged.");
    println!("  YOLO        Merge your own PR without a review.");
    println!(
        "GitHub controls qualification and when badges appear; this tool cannot guarantee attribution."
    );
    Ok(())
}

fn achievement_quickdraw(args: &[String]) -> Result<()> {
    let options = Options::parse(args)?;
    let repo = resolve_repo(options.repo.as_deref())?;
    validate_repo(&repo)?;
    ensure_repo_allowed(&repo, options.allow_private)?;
    let body = options.message.or(options.body).ok_or_else(|| {
        CliError("Quickdraw needs text: gh-achiv quickdraw -m \"issue details\"".into())
    })?;
    let title = options.title.unwrap_or_else(|| {
        let first = body
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Quickdraw issue")
            .trim();
        first.chars().take(80).collect()
    });
    if body.trim().is_empty() || title.trim().is_empty() {
        return Err(CliError(
            "Quickdraw requires non-empty issue text and title".into(),
        ));
    }
    println!("Quickdraw will create and close this issue in {repo}:\n  {title}\n  {body}");
    if !confirm("Proceed? Issue creation and closing are both part of this action.")? {
        println!("Cancelled; no remote changes made.");
        return Ok(());
    }
    let request_started = Instant::now();
    let url = gh_mutation(&[
        "issue", "create", "--repo", &repo, "--title", &title, "--body", &body,
    ])?;
    println!("Created: {url}");
    if request_started.elapsed() >= QUICKDRAW_CUTOFF {
        println!("Left open because the Quickdraw time window is nearly over.");
        return Ok(());
    }
    let pause = QUICKDRAW_PAUSE.saturating_sub(request_started.elapsed());
    if !pause.is_zero() {
        println!("Waiting {} seconds before closing…", pause.as_secs());
        thread::sleep(pause);
    }
    gh_mutation(&["issue", "close", &url])?;
    println!("Issue closed. GitHub decides whether this qualifies for Quickdraw.");
    Ok(())
}

fn achievement_merge(args: &[String], yolo: bool) -> Result<()> {
    let options = Options::parse(args)?;
    let repo = resolve_repo(options.repo.as_deref())?;
    validate_repo(&repo)?;
    ensure_repo_allowed(&repo, options.allow_private)?;
    let login = gh_text(&["api", "user", "--jq", ".login"])?;
    let mut number = options.number;
    let mut create_from_branch = false;
    let mut branch = String::new();
    let mut base = String::new();

    if number.is_none() {
        branch = git_text(&["branch", "--show-current"])?;
        if branch.is_empty() {
            return Err(CliError(
                "Run this from a named branch in your GitHub checkout.".into(),
            ));
        }
        let candidates = gh_text(&[
            "pr",
            "list",
            "--repo",
            &repo,
            "--head",
            &branch,
            "--state",
            "open",
            "--json",
            "number",
            "--jq",
            "[.[].number] | @tsv",
        ])?;
        let mut candidate_numbers = candidates.split_whitespace();
        number = match (candidate_numbers.next(), candidate_numbers.next()) {
            (None, _) => None,
            (Some(_), Some(_)) => {
                return Err(CliError(
                    "More than one open PR uses this branch; select one with --number.".into(),
                ));
            }
            (Some(number), None) => Some(number.to_owned()),
        };
        if number.is_none() {
            let text = options
                .message
                .as_deref()
                .or(options.body.as_deref())
                .ok_or_else(|| {
                    CliError(
                        "No open PR on this branch. Supply its text with -m to create one.".into(),
                    )
                })?;
            base = gh_text(&[
                "repo",
                "view",
                &repo,
                "--json",
                "defaultBranchRef",
                "--jq",
                ".defaultBranchRef.name",
            ])?;
            let diff = git_diff(&base, "HEAD")?;
            if diff.trim().is_empty() {
                return Err(CliError(format!(
                    "No changes between {base} and HEAD; cannot create a PR."
                )));
            }
            create_from_branch = true;
            let kind = if yolo { "YOLO" } else { "Pull Shark" };
            println!(
                "{kind} will create a PR in {repo}: {branch} → {base}\nText: {text}\n\nDiff:\n{diff}"
            );
        }
    }

    let message = options.message.or(options.body).unwrap_or_default();
    let mut title = options.title.unwrap_or_else(|| {
        branch
            .rsplit('/')
            .next()
            .unwrap_or(&branch)
            .replace(['-', '_'], " ")
    });
    if create_from_branch && title.trim().is_empty() {
        title = "Changes from current branch".into();
    }
    let mut number = number.unwrap_or_default();
    if !create_from_branch {
        if !number.chars().all(|ch| ch.is_ascii_digit()) || number == "0" {
            return Err(CliError(
                "--number must be a positive pull request number".into(),
            ));
        }
        let author = gh_text(&[
            "pr",
            "view",
            &number,
            "--repo",
            &repo,
            "--json",
            "author",
            "--jq",
            ".author.login",
        ])?;
        if author != login {
            return Err(CliError(format!(
                "PR #{number} is authored by {author}, not {login}"
            )));
        }
        let state = gh_text(&[
            "pr", "view", &number, "--repo", &repo, "--json", "state", "--jq", ".state",
        ])?;
        if state != "OPEN" {
            return Err(CliError(format!(
                "PR #{number} is {state}; only open PRs can be merged"
            )));
        }
        title = gh_text(&[
            "pr", "view", &number, "--repo", &repo, "--json", "title", "--jq", ".title",
        ])?;
    }
    if yolo && !create_from_branch {
        let reviews = gh_text(&[
            "pr",
            "view",
            &number,
            "--repo",
            &repo,
            "--json",
            "reviews",
            "--jq",
            ".reviews | length",
        ])?;
        if reviews != "0" {
            return Err(CliError(format!(
                "PR #{number} has {reviews} review(s); YOLO requires no reviews"
            )));
        }
    }
    let kind = if yolo { "YOLO" } else { "Pull Shark" };
    if !create_from_branch {
        println!("{kind} will merge your PR #{number} in {repo}: {title}");
    }
    if !confirm("Proceed? This may create and merge a pull request.")? {
        println!("Cancelled; no remote changes made.");
        return Ok(());
    }
    if create_from_branch {
        let text = if message.trim().is_empty() {
            &title
        } else {
            &message
        };
        let url = gh_mutation(&[
            "pr", "create", "--repo", &repo, "--base", &base, "--head", &branch, "--title", &title,
            "--body", text,
        ])?;
        println!("Created: {url}");
        number = url.rsplit('/').next().unwrap_or("").to_owned();
        if number.is_empty() {
            return Err(CliError(format!(
                "Created PR but couldn't read its number from: {url}"
            )));
        }
    }
    gh_mutation(&["pr", "merge", &number, "--repo", &repo, "--merge"])?;
    println!("Merged. GitHub decides whether this qualifies for {kind}.");
    Ok(())
}

fn resolve_repo(repo: Option<&str>) -> Result<String> {
    match repo {
        Some(repo) => Ok(repo.to_owned()),
        None => gh_text(&[
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "--jq",
            ".nameWithOwner",
        ])
        .map_err(|_| {
            CliError(
                "Cannot detect repository. Run inside a GitHub checkout or pass --repo OWNER/REPO."
                    .into(),
            )
        }),
    }
}

fn git_text(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|error| CliError(format!("could not run git: {error}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(CliError(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn create_pr(args: &[String]) -> Result<()> {
    let options = Options::parse(args)?;
    let repo = Options::require(&options.repo, "--repo")?;
    validate_repo(&repo)?;
    ensure_repo_allowed(&repo, options.allow_private)?;
    let base = Options::require(&options.base, "--base")?;
    let head = Options::require(&options.head, "--head")?;
    let title = Options::require(&options.title, "--title")?;
    let body = Options::require(&options.body, "--body")?;

    let diff = git_diff(&base, &head)?;
    if diff.trim().is_empty() {
        return Err(CliError(format!(
            "no local changes found between `{base}` and `{head}`; refusing to create an empty PR"
        )));
    }
    println!(
        "Proposed PR in {repo}: {head} → {base}\nTitle: {title}\nBody: {body}\n\nDiff:\n{diff}"
    );
    if !confirm("Create this pull request?")? {
        println!("Cancelled; no remote changes made.");
        return Ok(());
    }
    let url = gh_mutation(&[
        "pr", "create", "--repo", &repo, "--base", &base, "--head", &head, "--title", &title,
        "--body", &body,
    ])?;
    println!("Created: {url}");
    Ok(())
}

fn merge_pr(args: &[String]) -> Result<()> {
    let options = Options::parse(args)?;
    let repo = Options::require(&options.repo, "--repo")?;
    validate_repo(&repo)?;
    ensure_repo_allowed(&repo, options.allow_private)?;
    let number = Options::require(&options.number, "--number")?;
    if !number.chars().all(|ch| ch.is_ascii_digit()) || number == "0" {
        return Err(CliError(
            "--number must be a positive pull request number".into(),
        ));
    }
    let login = gh_text(&["api", "user", "--jq", ".login"])?;
    let author = gh_text(&[
        "pr",
        "view",
        &number,
        "--repo",
        &repo,
        "--json",
        "author",
        "--jq",
        ".author.login",
    ])?;
    if author != login {
        return Err(CliError(format!(
            "PR #{number} is authored by {author}, not the authenticated user {login}"
        )));
    }
    let state = gh_text(&[
        "pr", "view", &number, "--repo", &repo, "--json", "state", "--jq", ".state",
    ])?;
    if state != "OPEN" {
        return Err(CliError(format!(
            "PR #{number} is {state}; only open PRs can be merged"
        )));
    }
    let reviews = gh_text(&[
        "pr",
        "view",
        &number,
        "--repo",
        &repo,
        "--json",
        "reviews",
        "--jq",
        ".reviews | length",
    ])?;
    if reviews != "0" {
        return Err(CliError(format!(
            "PR #{number} has {reviews} review(s); refusing the no-review YOLO action"
        )));
    }
    let title = gh_text(&[
        "pr", "view", &number, "--repo", &repo, "--json", "title", "--jq", ".title",
    ])?;
    println!("Will merge your own PR #{number} in {repo} without a review: {title}");
    if !confirm("Merge this pull request? This cannot be undone. Continue?")? {
        println!("Cancelled; no remote changes made.");
        return Ok(());
    }
    gh_mutation(&["pr", "merge", &number, "--repo", &repo, "--merge"])?;
    println!("Merged. GitHub decides whether this qualifies for YOLO.");
    Ok(())
}

fn validate_repo(repo: &str) -> Result<()> {
    let mut parts = repo.split('/');
    let valid = parts.next().is_some_and(valid_repo_part)
        && parts.next().is_some_and(valid_repo_part)
        && parts.next().is_none();
    if !valid {
        return Err(CliError("--repo must use OWNER/REPO format".into()));
    }
    Ok(())
}

fn valid_repo_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
}

fn ensure_repo_allowed(repo: &str, allow_private: bool) -> Result<()> {
    let visibility = gh_text(&["api", &format!("repos/{repo}"), "--jq", ".visibility"])?;
    if visibility != "public" && !allow_private {
        return Err(CliError(format!(
            "{repo} is {visibility}; pass --allow-private to proceed explicitly"
        )));
    }
    Ok(())
}

fn git_diff(base: &str, head: &str) -> Result<String> {
    let output = Command::new("git")
        .args([
            "diff",
            "--no-ext-diff",
            "--no-color",
            &format!("{base}...{head}"),
        ])
        .output()
        .map_err(|error| CliError(format!("could not run git: {error}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(CliError(format!(
        "could not inspect local diff between `{base}` and `{head}`: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    io::stdout()
        .flush()
        .map_err(|error| CliError(error.to_string()))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| CliError(error.to_string()))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn gh_text(args: &[&str]) -> Result<String> {
    let output = gh_output(args)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn gh_mutation(args: &[&str]) -> Result<String> {
    let permit = ActionPermit::acquire()?;
    let result = gh_output(args);
    permit.record_completion()?;
    let output = result.map_err(|error| {
        let detail = error.0;
        if is_rate_limit_error(&detail) {
            CliError(format!(
                "{detail}\nStopped without retrying to avoid duplicate actions. Wait at least one minute, or longer if GitHub supplied a retry/reset time, then try again."
            ))
        } else {
            CliError(detail)
        }
    })?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn is_rate_limit_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("rate limit")
        || message.contains("secondary rate")
        || message.contains("retry-after")
        || message.contains("429")
        || message.contains("abuse detection")
}

struct ActionPermit {
    _lock: File,
    lock_path: PathBuf,
}

impl ActionPermit {
    fn acquire() -> Result<Self> {
        let state_dir = state_dir()?;
        fs::create_dir_all(&state_dir).map_err(|error| {
            CliError(format!(
                "cannot create safety state directory {}: {error}",
                state_dir.display()
            ))
        })?;
        let lock_path = state_dir.join("action.lock");
        let lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    CliError(format!(
                        "another gh-achievements action is running (or a stale lock exists at {}). Wait for it to finish; if no process is running, remove that lock file manually",
                        lock_path.display()
                    ))
                } else {
                    CliError(format!("cannot acquire action safety lock: {error}"))
                }
            })?;
        let permit = Self {
            _lock: lock,
            lock_path,
        };
        let state_path = state_dir.join("rate-state");
        let now = unix_millis()?;
        let last_action = read_rate_state(&state_path)?;
        if last_action > 0 {
            let elapsed = now.saturating_sub(last_action);
            let minimum = MIN_MUTATION_INTERVAL.as_millis() as u64;
            if elapsed < minimum {
                let wait = minimum - elapsed;
                eprintln!("Waiting {} ms between GitHub changes...", wait);
                thread::sleep(Duration::from_millis(wait));
            }
        }
        write_rate_state(&state_path, unix_millis()?)?;
        Ok(permit)
    }

    fn record_completion(&self) -> Result<()> {
        write_rate_state(&state_dir()?.join("rate-state"), unix_millis()?)
    }
}

impl Drop for ActionPermit {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

fn state_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path).join("gh-achievements"));
    }
    let home = env::var_os("HOME").ok_or_else(|| {
        CliError("HOME or XDG_STATE_HOME is required for persistent safety limits".into())
    })?;
    Ok(PathBuf::from(home).join(".local/state/gh-achievements"))
}

fn unix_millis() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .map_err(|error| CliError(format!("system clock is before Unix epoch: {error}")))
}

fn read_rate_state(path: &PathBuf) -> Result<u64> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(CliError(format!("cannot read safety state: {error}"))),
    };
    let mut values = contents.split_whitespace();
    let last = parse_state_value(values.next(), "last action")?;
    if values.next().is_some() {
        return Err(CliError(
            "safety state is malformed; refusing to bypass safety limits".into(),
        ));
    }
    Ok(last)
}

fn parse_state_value<T: std::str::FromStr>(value: Option<&str>, label: &str) -> Result<T> {
    value
        .ok_or_else(|| {
            CliError("safety state is incomplete; refusing to bypass safety limits".into())
        })?
        .parse()
        .map_err(|_| {
            CliError(format!(
                "safety state {label} is invalid; refusing to bypass safety limits"
            ))
        })
}

fn write_rate_state(path: &PathBuf, last: u64) -> Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = File::create(&temporary)
        .map_err(|error| CliError(format!("cannot write safety state: {error}")))?;
    writeln!(file, "{last}")
        .map_err(|error| CliError(format!("cannot write safety state: {error}")))?;
    file.sync_all()
        .map_err(|error| CliError(format!("cannot sync safety state: {error}")))?;
    fs::rename(&temporary, path)
        .map_err(|error| CliError(format!("cannot save safety state: {error}")))
}

fn gh_output(args: &[&str]) -> Result<Output> {
    let output = Command::new("gh").args(args).output().map_err(|error| {
        CliError(format!(
            "could not run GitHub CLI `gh`: {error}; install it and run `gh auth login`"
        ))
    })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(CliError(format!(
            "gh {} failed: {}",
            args.first().copied().unwrap_or("command"),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}
