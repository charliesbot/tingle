//! tingle — fast, stateless orientation map for AI agents.
//!
//! Usage:
//!
//! ```text
//! tingle [REPO]
//! tingle --alias PREFIX:PATH [REPO]
//! tingle --no-legend [REPO]
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use clap::Parser;
use time::OffsetDateTime;

use tingle::{enumerate, manifest, parse, rank, render, resolve};

#[derive(Parser, Debug)]
#[command(
    name = "tingle",
    version,
    about = "Fast, stateless orientation map for AI agents"
)]
struct Args {
    /// Repo path (default: .)
    #[arg(value_name = "REPO")]
    repo: Option<PathBuf>,

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
    let repo = args.repo.unwrap_or_else(|| PathBuf::from("."));
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

    // 1. enumerate
    let mut files = match enumerate::repo(&repo_abs) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("tingle: {}", e);
            return ExitCode::from(1);
        }
    };

    // 2. parse
    parse::new_run();
    parse::all(&repo_abs, &mut files, &parse::PACKAGE_STATS);

    // 3. resolve
    resolve::all(&mut files, &aliases);

    // 4. rank
    let g = rank::graph(&mut files);

    // 4a. manifest
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

    // 5. render
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
    };

    let out = render::render(
        &files,
        &entries,
        &utilities,
        &g.dir_edges,
        &g.callers,
        &m.s_records,
        &opts,
    );
    print!("{}", out);

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

fn short_git_commit(repo: &std::path::Path) -> String {
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
