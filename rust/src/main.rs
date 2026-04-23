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
//! tingle --stdout [REPO]          # print map to stdout (for pipelines)
//! tingle --alias PREFIX:PATH ...  # import alias substitution (TS/webpack)
//! ```
//!
//! No output-shape toggles. One rich default: def signatures, full
//! import lists, up to 10 callers per utility. File-based consumption
//! has no token cap (agents `Read` it directly), so truncation earns
//! nothing. If the repo is so large that the default is too much,
//! run tingle on a subdirectory: `tingle features/feed`.

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

    /// Map an import prefix to a repo path; repeatable (e.g. `--alias '@:src'`).
    /// Needed for TypeScript projects that use `tsconfig.json`
    /// `paths` — without it, `@/foo` imports stay unresolved.
    #[arg(long = "alias", value_name = "PREFIX:PATH", action = clap::ArgAction::Append)]
    alias: Vec<String>,
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

    let hotspots = rank::hotspots(
        &files,
        rank::HotspotsOpts {
            repo: &repo_abs,
            manifest_ep: &m.entry_targets,
            max_hotspots: 15,
        },
    );
    let utilities = rank::utilities(&files, &g.callers);

    let gen_date = OffsetDateTime::now_utc()
        .format(&time::format_description::parse("[year]-[month]-[day]").unwrap())
        .unwrap_or_default();

    let opts = render::Options {
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: short_git_commit(&repo_abs),
        tokenizer_id: "cl100k_base".to_string(),
        gen_date,
        suppress_warning: !args.stdout,
    };

    let map = render::render(
        &files,
        &hotspots,
        &utilities,
        &g.dir_edges,
        &g.callers,
        &m.s_records,
        &opts,
    );

    // Decide output destination. With `--stdout` the map goes straight
    // to stdout (for pipelines). Otherwise write to
    // `<REPO>/.tinglemap.md` and emit a one-line status — downstream
    // tools can detect the path without parsing the map.
    if args.stdout {
        print!("{}", map);
    } else {
        let out_path = repo_abs.join(DEFAULT_OUTPUT_FILE);
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
