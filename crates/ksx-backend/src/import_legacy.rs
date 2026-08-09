//! `ksx import-legacy` — turn a Keyboard Splitter Xbox profile folder into TOML.
//!
//! The one verb whose body was still in `main.rs` after the ksx-app split, and
//! only because it had always been a free function beside `fn main` rather than
//! a module. It is a verb like every other: a directory in, a rendered report
//! and a set of writes out.

/// Exit code 3 = import completed but produced warnings (0 = clean, 1 = error).
const EXIT_IMPORT_WARNINGS: i32 = 3;

pub fn run(from: Option<std::path::PathBuf>, dry_run: bool, json: bool) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use ksx_legacy_import::json_escape;

    let dir = from.unwrap_or_else(ksx_legacy_import::default_legacy_dir);
    let import = ksx_legacy_import::import_dir(&dir)
        .with_context(|| format!("importing legacy XML from '{}'", dir.display()))?;

    if dry_run {
        let files = import.rendered_files()?;
        if json {
            let rendered: Vec<String> = files
                .iter()
                .map(|f| {
                    format!(
                        "{{\"path\":\"{}\",\"content\":\"{}\"}}",
                        json_escape(&f.path),
                        json_escape(&f.content)
                    )
                })
                .collect();
            println!(
                "{{\"dry_run\":true,\"legacy_dir\":\"{}\",\"report\":{},\"files\":[{}]}}",
                json_escape(&dir.display().to_string()),
                import.report.to_json(),
                rendered.join(",")
            );
        } else {
            for file in &files {
                println!("==== {} ====", file.path);
                println!("{}", file.content);
            }
            // Report on stderr so stdout stays pipeable TOML.
            eprintln!("{}", import.report);
        }
    } else {
        let root = ksx_legacy_import::default_config_root()
            .context("cannot resolve the ksx config root (%APPDATA% is not set)")?;
        let written = import
            .write_outputs(&root)
            .with_context(|| format!("writing TOML into '{}'", root.display()))?;
        if json {
            let paths: Vec<String> = written
                .iter()
                .map(|p| format!("\"{}\"", json_escape(&p.display().to_string())))
                .collect();
            println!(
                "{{\"dry_run\":false,\"legacy_dir\":\"{}\",\"config_root\":\"{}\",\"report\":{},\"written\":[{}]}}",
                json_escape(&dir.display().to_string()),
                json_escape(&root.display().to_string()),
                import.report.to_json(),
                paths.join(",")
            );
        } else {
            println!("{}", import.report);
            for path in &written {
                println!("wrote {}", path.display());
            }
        }
    }

    if !import.report.warnings.is_empty() {
        std::process::exit(EXIT_IMPORT_WARNINGS);
    }
    Ok(())
}
