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
    match command.as_str() {
        "status" => status(&rest),
        "quickdraw" => quickdraw(&rest),
        "pr" => create_pr(&rest),
        "merge" => merge_pr(&rest),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(CliError(format!("unknown command `{other}`\n\n{HELP}"))),
    }
}

const HELP: &str = "Usage:
  gh-achievements status
  gh-achievements quickdraw --repo OWNER/REPO --title TITLE --body BODY [--allow-private]
  gh-achievements pr --repo OWNER/REPO --base BRANCH --head BRANCH --title TITLE --body BODY [--allow-private]
  gh-achievements merge --repo OWNER/REPO --number PR_NUMBER [--allow-private]

All remote changes require confirmation. Quickdraw closes the issue only after a
second confirmation. PR creation requires a real local diff between base and head.
Repositories must be public unless --allow-private is supplied. Safety limits:
at least two seconds between mutating requests; requests are serialized.";

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
            if !key.starts_with("--") {
                return Err(CliError(format!("unexpected argument `{key}`")));
            }
            let value = args
                .get(i + 1)
                .ok_or_else(|| CliError(format!("missing value for {key}")))?
                .clone();
            match key {
                "--repo" => options.repo = Some(value),
                "--title" => options.title = Some(value),
                "--body" => options.body = Some(value),
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

fn quickdraw(args: &[String]) -> Result<()> {
    let options = Options::parse(args)?;
    let repo = Options::require(&options.repo, "--repo")?;
    validate_repo(&repo)?;
    ensure_repo_allowed(&repo, options.allow_private)?;
    let title = Options::require(&options.title, "--title")?;
    let body = Options::require(&options.body, "--body")?;
    println!("Will create an issue in {repo}:\n  Title: {title}\n  Body: {body}");
    if !confirm("Create this issue?")? {
        println!("Cancelled; no remote changes made.");
        return Ok(());
    }
    let url = gh_mutation(&[
        "issue", "create", "--repo", &repo, "--title", &title, "--body", &body,
    ])?;
    let created_at = Instant::now();
    println!("Created: {url}");
    if confirm("Close this issue now?")? {
        let elapsed = created_at.elapsed();
        if elapsed >= QUICKDRAW_CUTOFF {
            println!(
                "Left open: more than four minutes have passed, so closing now would be too close to the five-minute qualification window."
            );
            return Ok(());
        }
        let remaining_pause = QUICKDRAW_PAUSE.saturating_sub(elapsed);
        if !remaining_pause.is_zero() {
            println!(
                "Waiting {} seconds before closing to avoid back-to-back writes...",
                remaining_pause.as_secs()
            );
            thread::sleep(remaining_pause);
        }
        gh_mutation(&["issue", "close", &url])?;
        println!("Closed. GitHub decides whether this qualifies for Quickdraw.");
    } else {
        println!("Issue left open: {url}");
    }
    Ok(())
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
    if !confirm_exact("Type MERGE to continue:", "MERGE")? {
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

fn confirm_exact(prompt: &str, expected: &str) -> Result<bool> {
    print!("{prompt} ");
    io::stdout()
        .flush()
        .map_err(|error| CliError(error.to_string()))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| CliError(error.to_string()))?;
    Ok(answer.trim() == expected)
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
