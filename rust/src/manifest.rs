//! package.json + go.mod summary records.
//!
//! Mirrors `internal/manifest/manifest.go`. Surfaces only fields useful for
//! agent orientation: scripts, bin, main/module from package.json; module
//! path and go version from go.mod.

use std::fs;
use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

#[derive(Default, Debug)]
pub struct Parsed {
    pub s_records: Vec<String>,
    pub entry_targets: Vec<String>,
}

/// Scan the repo root for `package.json` and `go.mod`. Missing files are
/// silently skipped; `scan` never errors.
pub fn scan(repo: &Path) -> Parsed {
    let mut out = Parsed::default();
    scan_package_json(repo, &mut out);
    scan_go_mod(repo, &mut out);
    out
}

#[derive(Deserialize, Default)]
struct PackageJson {
    #[serde(default)]
    main: String,
    #[serde(default)]
    module: String,
    #[serde(default)]
    bin: Option<Value>,
    #[serde(default)]
    scripts: std::collections::HashMap<String, String>,
}

fn scan_package_json(repo: &Path, out: &mut Parsed) {
    let Ok(data) = fs::read_to_string(repo.join("package.json")) else {
        return;
    };
    let Ok(pkg): Result<PackageJson, _> = serde_json::from_str(&data) else {
        return;
    };

    if !pkg.scripts.is_empty() {
        let mut keys: Vec<&String> = pkg.scripts.keys().collect();
        keys.sort();
        let mut parts: Vec<String> = Vec::with_capacity(keys.len());
        for k in keys {
            parts.push(format!("{}={}", k, summarize_script(&pkg.scripts[k])));
        }
        out.s_records
            .push(format!("S package.json  scripts: {}", parts.join(" ")));
    }

    match (!pkg.main.is_empty(), !pkg.module.is_empty()) {
        (true, true) => {
            out.s_records.push(format!(
                "S package.json  main: {}  module: {}",
                pkg.main, pkg.module
            ));
            out.entry_targets.push(pkg.main.clone());
            out.entry_targets.push(pkg.module.clone());
        }
        (true, false) => {
            out.s_records
                .push(format!("S package.json  main: {}", pkg.main));
            out.entry_targets.push(pkg.main.clone());
        }
        (false, true) => {
            out.s_records
                .push(format!("S package.json  module: {}", pkg.module));
            out.entry_targets.push(pkg.module.clone());
        }
        _ => {}
    }

    if let Some(bin_val) = &pkg.bin {
        let bins = parse_bin(bin_val);
        if !bins.is_empty() {
            let mut parts: Vec<String> = Vec::with_capacity(bins.len());
            for (name, path) in &bins {
                parts.push(format!("{}->{}", name, path));
                out.entry_targets.push(path.clone());
            }
            out.s_records
                .push(format!("S package.json  bin: {}", parts.join(" ")));
        }
    }
}

fn parse_bin(raw: &Value) -> Vec<(String, String)> {
    if let Some(s) = raw.as_str() {
        if !s.is_empty() {
            return vec![("default".into(), s.to_string())];
        }
    }
    if let Some(obj) = raw.as_object() {
        let mut out: Vec<(String, String)> = obj
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        return out;
    }
    Vec::new()
}

/// Match Go's `summarizeScript`: collapse whitespace, cap at 40 chars with `…`.
fn summarize_script(s: &str) -> String {
    let s = s.replace('\n', " ");
    let s = COLLAPSE_WS.replace_all(&s, " ").into_owned();
    if s.chars().count() > 40 {
        let truncated: String = s.chars().take(40).collect();
        format!("{}…", truncated.trim_end())
    } else {
        s
    }
}

static COLLAPSE_WS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());

fn scan_go_mod(repo: &Path, out: &mut Parsed) {
    let Ok(data) = fs::read_to_string(repo.join("go.mod")) else {
        return;
    };
    let mut module = String::new();
    let mut go_ver = String::new();
    for line in data.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("module ") {
            module = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("go ") {
            go_ver = rest.trim().to_string();
        }
    }
    match (!module.is_empty(), !go_ver.is_empty()) {
        (true, true) => out
            .s_records
            .push(format!("S go.mod        module={}  go={}", module, go_ver)),
        (true, false) => out
            .s_records
            .push(format!("S go.mod        module={}", module)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn package_json_scripts_main_bin() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("package.json"),
            r#"{
              "name": "x",
              "main": "dist/index.js",
              "scripts": { "build": "tsc -p .", "test": "jest" },
              "bin": { "mytool": "dist/cli.js" }
            }"#,
        )
        .unwrap();
        let out = scan(root);
        assert_eq!(
            out.s_records,
            vec![
                "S package.json  scripts: build=tsc -p . test=jest".to_string(),
                "S package.json  main: dist/index.js".to_string(),
                "S package.json  bin: mytool->dist/cli.js".to_string(),
            ]
        );
        assert!(out.entry_targets.contains(&"dist/index.js".to_string()));
        assert!(out.entry_targets.contains(&"dist/cli.js".to_string()));
    }

    #[test]
    fn bin_as_string_yields_default() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("package.json"),
            r#"{"name":"x","bin":"dist/cli.js"}"#,
        )
        .unwrap();
        let out = scan(root);
        assert!(out
            .s_records
            .iter()
            .any(|r| r == "S package.json  bin: default->dist/cli.js"));
    }

    #[test]
    fn go_mod_module_and_go_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("go.mod"), "module github.com/x/y\n\ngo 1.22\n").unwrap();
        let out = scan(root);
        assert_eq!(
            out.s_records,
            vec!["S go.mod        module=github.com/x/y  go=1.22".to_string()]
        );
    }

    #[test]
    fn summarize_script_caps_long_bodies() {
        let long = "a".repeat(60);
        let s = summarize_script(&long);
        assert!(s.ends_with('…'), "{}", s);
    }
}
