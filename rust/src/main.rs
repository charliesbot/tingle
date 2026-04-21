//! tingle — orientation map for AI agents.
//!
//! Default behavior writes to `<repo>/.tinglemap.md` — agents `Read` the
//! file instead of parsing tingle's stdout, dodging Bash-tool preview
//! caps entirely. Every invocation regenerates from scratch (no cache;
//! sub-second parse time makes correctness cheaper than invalidation).
//!
//! Usage:
//!
//! ```text
//! tingle [REPO]                   # write .tinglemap.md; print status line
//! tingle --stdout [REPO]          # print map to stdout (old default)
//! tingle --out PATH [REPO]        # write to PATH instead of .tinglemap.md
//! tingle --alias PREFIX:PATH ...  # import alias substitution
//! tingle --scope PATH ...         # filter F section to subtree
//! tingle --skeleton ...           # drop F section (architecture only)
//! tingle --full ...               # include per-file def signatures
//! tingle --no-legend ...          # skip the legend line
//! ```

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Parser;
use time::OffsetDateTime;

use tingle::{enumerate, manifest, parse, rank, render, resolve};

const DEFAULT_OUTPUT_FILE: &str = ".tinglemap.md";

#[derive(Parser, Debug)]
#[command(
    name = "tingle",
    version,
    about = "Orientation map for AI agents (writes .tinglemap.md by default)"
)]
struct Args {
    /// Repo path (default: .)
    #[arg(value_name = "REPO")]
    repo: Option<PathBuf>,

    /// Print the map to stdout instead of writing a file. Use for shell
    /// pipelines (e.g. `tingle --stdout | jq`). Without this flag, the
    /// map is written to `<REPO>/.tinglemap.md` and only a status line
    /// hits stdout.
    #[arg(long = "stdout")]
    stdout: bool,

    /// Write the map to PATH instead of the default
    /// `<REPO>/.tinglemap.md`. Use this only if you want a different
    /// location than the default — most agents should just rely on
    /// `<REPO>/.tinglemap.md` and `Read('./.tinglemap.md')`. Ignored
    /// when `--stdout` is set.
    #[arg(long = "out", value_name = "PATH")]
    out: Option<PathBuf>,

    /// Map an import prefix to a repo path; repeatable (e.g. `--alias '@:src'`).
    #[arg(long = "alias", value_name = "PREFIX:PATH", action = clap::ArgAction::Append)]
    alias: Vec<String>,

    /// Omit the legend header line.
    #[arg(long = "no-legend")]
    no_legend: bool,

    /// Filter the Files section to paths under PATH. Top sections
    /// (Manifests, Entry points, Utilities, Modules) still render
    /// whole-repo context.
    #[arg(long = "scope", value_name = "PATH")]
    scope: Option<String>,

    /// Omit the Files section entirely — emit only the architecture
    /// layer (manifests, entries, utilities, module graph).
    #[arg(long = "skeleton")]
    skeleton: bool,

    /// Include per-file def listings in the Files section AND show up to 3
    /// callers per Utility record. Default is the compact layout
    /// (paths/imports/tags only, 1 caller per U), which preserves agent
    /// task quality (eval mean ≥0.97 across 3 real repos) at 47-58% of
    /// the token cost.
    #[arg(long = "full")]
    full: bool,

    /// Deprecated: compact is now the default. Accepted as a no-op for
    /// backwards compatibility with older scripts.
    #[arg(long = "compact", hide = true)]
    _compat_compact: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let repo = args.repo.clone().unwrap_or_else(|| PathBuf::from("."));
    let repo_abs = match repo.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("tingle: {}", e);
            return ExitCode::from(1);
        }
    };
    if !repo_abs.is_dir() {
        eprintln!("tingle: not a directory: {}", repo_abs.display());
        return ExitCode::from(1);
    }

    let mut aliases: HashMap<String, String> = HashMap::new();
    for spec in &args.alias {
        let Some(idx) = spec.find(':') else {
            eprintln!("tingle: alias must be PREFIX:PATH, got {:?}", spec);
            return ExitCode::from(1);
        };
        aliases.insert(spec[..idx].to_string(), spec[idx + 1..].to_string());
    }

    // Pipeline: enumerate → parse → resolve → rank → render.
    let mut files = match enumerate::repo(&repo_abs) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("tingle: {}", e);
            return ExitCode::from(1);
        }
    };

    parse::new_run();
    parse::all(&repo_abs, &mut files, &parse::PACKAGE_STATS);
    resolve::all(&mut files, &aliases);
    let g = rank::graph(&mut files);
    let m = manifest::scan(&repo_abs);

    let entries = rank::entry_points(
        &files,
        rank::EntryPointsOpts {
            repo: &repo_abs,
            manifest_ep: &m.entry_targets,
            max_eps: 15,
        },
    );
    let utilities = rank::utilities(&files);

    let gen_date = OffsetDateTime::now_utc()
        .format(&time::format_description::parse("[year]-[month]-[day]").unwrap())
        .unwrap_or_default();

    let opts = render::Options {
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: short_git_commit(&repo_abs),
        tokenizer_id: "cl100k_base".to_string(),
        no_legend: args.no_legend,
        tokens_approx: 0,
        gen_date,
        scope: args.scope.unwrap_or_default(),
        skeleton: args.skeleton,
        full: args.full,
        suppress_warning: !args.stdout,
    };

    let map = render::render(
        &files,
        &entries,
        &utilities,
        &g.dir_edges,
        &g.callers,
        &m.s_records,
        &opts,
    );

    // Decide output destination: --stdout wins over --out; --out wins
    // over default. When writing a file, stdout gets a one-line status
    // instead of the map (so downstream tools can log / detect the
    // path without parsing the map).
    if args.stdout {
        print!("{}", map);
    } else {
        let out_path = args
            .out
            .unwrap_or_else(|| repo_abs.join(DEFAULT_OUTPUT_FILE));
        match write_atomic(&out_path, map.as_bytes()) {
            Ok(()) => {
                let bytes = map.len();
                let tokens_k = (bytes as f64 / 4000.0).max(0.1);
                let rel_display = display_path(&out_path, &repo_abs);
                // The trailing `Read(...)` hint is for AI agents — many
                // jump straight from `tingle`'s stdout to the next tool
                // call; spelling out the literal next command shaves
                // a turn off the loop.
                println!(
                    "wrote {} ({} bytes, ~{:.1}k tokens). Next: Read('{}')",
                    rel_display, bytes, tokens_k, rel_display
                );
                maybe_gitignore_hint(&out_path, &repo_abs);
            }
            Err(e) => {
                eprintln!("tingle: failed to write {}: {}", out_path.display(), e);
                return ExitCode::from(1);
            }
        }
    }

    use std::sync::atomic::Ordering;
    let perr = parse::PACKAGE_STATS.parse_errors.load(Ordering::Relaxed);
    if perr > 0 {
        eprintln!(
            "tingle: {} parse errors (files emitted without defs/imports)",
            perr
        );
    }
    let rerr = parse::PACKAGE_STATS.read_errors.load(Ordering::Relaxed);
    if rerr > 0 {
        eprintln!("tingle: {} files unreadable", rerr);
    }

    ExitCode::SUCCESS
}

/// Write atomically: to `path.tmp.<pid>`, then rename. Prevents
/// torn reads when two tingle invocations race in the same CWD.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_file_name(format!(
        "{}.tmp.{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(DEFAULT_OUTPUT_FILE),
        std::process::id()
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Format a path for the status line: relative to the repo when inside,
/// else absolute. Keeps the common case short.
fn display_path(path: &Path, repo: &Path) -> String {
    match path.strip_prefix(repo) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// If the output file is being written inside the repo and the repo's
/// `.gitignore` doesn't mention it, nudge the user once per run. Crude
/// substring check — gitignore has glob semantics we don't fully parse,
/// but catches the common case of "user forgot to add this line."
fn maybe_gitignore_hint(out_path: &Path, repo: &Path) {
    let Ok(rel) = out_path.strip_prefix(repo) else {
        return; // output is outside the repo; not our business
    };
    let rel_str = rel.display().to_string();
    let gitignore = repo.join(".gitignore");
    let Ok(content) = std::fs::read_to_string(&gitignore) else {
        return; // no .gitignore → not a git repo, or user manages it elsewhere
    };
    // Look for the exact file name OR a leading-dot pattern that covers it.
    let covered = content.lines().any(|line| {
        let line = line.trim();
        line == rel_str
            || line == format!("/{}", rel_str)
            || line == DEFAULT_OUTPUT_FILE
            || line == format!("/{}", DEFAULT_OUTPUT_FILE)
    });
    if !covered {
        eprintln!(
            "tingle: hint — add `{}` to .gitignore (generated artifact; committing invites drift)",
            DEFAULT_OUTPUT_FILE
        );
    }
}

fn short_git_commit(repo: &Path) -> String {
    let out = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["rev-parse", "--short", "HEAD"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}
