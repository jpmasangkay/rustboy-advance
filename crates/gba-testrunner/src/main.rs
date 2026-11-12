use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use gba_testrunner::{
    RunOptions, discover_workspace_root, load_manifest, render_markdown_report, run_suite,
    summarize_results,
};

struct CliArgs {
    manifest_path: Option<PathBuf>,
    root_path: Option<PathBuf>,
    bios_path: Option<PathBuf>,
    report_path: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    filter: Option<String>,
    suite: Option<String>,
    record: bool,
    dump_all: bool,
    allow_failures: bool,
    fail_on_unexpected: bool,
}

fn parse_args() -> Result<CliArgs, String> {
    let mut cli = CliArgs {
        manifest_path: None,
        root_path: None,
        bios_path: None,
        report_path: None,
        out_dir: None,
        filter: None,
        suite: None,
        record: false,
        dump_all: false,
        allow_failures: false,
        fail_on_unexpected: false,
    };

    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest" => {
                let v = args.next().ok_or("missing value for --manifest")?;
                cli.manifest_path = Some(PathBuf::from(v));
            }
            "--root" => {
                let v = args.next().ok_or("missing value for --root")?;
                cli.root_path = Some(PathBuf::from(v));
            }
            "--bios" => {
                let v = args.next().ok_or("missing value for --bios")?;
                cli.bios_path = Some(PathBuf::from(v));
            }
            "--report" => {
                // Support both `--report` (defaults to TEST_RESULTS.md) and `--report <path>`.
                let next_is_path = args.peek().is_some_and(|s| !s.starts_with('-'));
                if next_is_path {
                    let v = args.next().ok_or("missing path for --report")?;
                    cli.report_path = Some(PathBuf::from(v));
                } else {
                    cli.report_path = Some(PathBuf::from("TEST_RESULTS.md"));
                }
            }
            "--out-dir" => {
                let v = args.next().ok_or("missing value for --out-dir")?;
                cli.out_dir = Some(PathBuf::from(v));
            }
            "--filter" => {
                let v = args.next().ok_or("missing value for --filter")?;
                cli.filter = Some(v);
            }
            "--suite" => {
                let v = args.next().ok_or("missing value for --suite")?;
                cli.suite = Some(v);
            }
            "--record" => cli.record = true,
            "--dump-all" => cli.dump_all = true,
            "--allow-failures" => cli.allow_failures = true,
            "--fail-on-unexpected" => cli.fail_on_unexpected = true,
            "--help" | "-h" => {
                println!(
                    "Usage: gba-testrunner [OPTIONS]\n\
                     Options:\n\
                     \t--manifest <path>       Path to manifest.toml\n\
                     \t--root <path>           Workspace root (auto-detected by default)\n\
                     \t--bios <path>           Optional 16KiB GBA BIOS image\n\
                     \t--suite <name>          Run only tests in a given suite\n\
                     \t--filter <substr>       Run only tests matching substring\n\
                     \t--record                (Re)generate golden hashes/PNGs in golden/ (skips known_failure & interactive)\n\
                     \t--report [path]         Write Markdown results (default: TEST_RESULTS.md)\n\
                     \t--out-dir <path>        Directory for failure PNGs (default: test_output)\n\
                     \t--dump-all              Save PNG screenshots for all executed ROMs\n\
                     \t--allow-failures        Exit 0 even if tests fail\n\
                     \t--fail-on-unexpected    Exit 1 only on regressions not marked known_failure"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    Ok(cli)
}

fn main() -> ExitCode {
    let cli = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let root = cli.root_path.unwrap_or_else(discover_workspace_root);
    let manifest = match load_manifest(cli.manifest_path.as_deref(), &root) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let mut opts = RunOptions::for_workspace(&root);
    if let Some(bios) = cli.bios_path {
        opts.bios_path = Some(bios);
    }
    if let Some(out) = cli.out_dir {
        opts.output_dir = out;
    }
    opts.record = cli.record;
    opts.dump_all = cli.dump_all;
    opts.filter = cli.filter;
    opts.suite = cli.suite;

    let results = match run_suite(&root, &manifest, &opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    for r in &results {
        println!(
            "[{:<21}] {:<28} ({:<10}) {:>6.3} ms | {}",
            r.outcome.status_text(),
            r.name,
            r.criterion.label(),
            r.avg_frame_ms,
            r.notes
        );
    }

    let counts = summarize_results(&results);
    println!(
        "\nSummary: {} passed, {} failed, {} skipped (missing ROM), {} skipped (missing golden)",
        counts.passed, counts.failed, counts.skipped_rom, counts.skipped_golden
    );

    if let Some(report_rel) = cli.report_path {
        let report_abs = if report_rel.is_absolute() {
            report_rel
        } else {
            root.join(report_rel)
        };
        let bios_label = opts
            .bios_path
            .as_ref()
            .map(|p| format!("LLE (`{}`)", p.display()))
            .unwrap_or_else(|| "HLE (built-in)".to_string());
        let mut md = render_markdown_report(&results, &bios_label);
        if let Ok(existing) = fs::read_to_string(&report_abs)
            && let Some(idx) = existing.find("\n## Failure Diagnosis")
        {
            md.push_str(&existing[idx..]);
        }
        if let Err(e) = fs::write(&report_abs, md) {
            eprintln!("error writing report {}: {e}", report_abs.display());
            return ExitCode::from(2);
        }
        println!("Wrote report to {}", report_abs.display());
    }

    let unexpected_failures = results
        .iter()
        .filter(|r| r.outcome == gba_testrunner::TestOutcome::Fail && !r.known_failure)
        .count();

    if cli.allow_failures {
        ExitCode::SUCCESS
    } else if cli.fail_on_unexpected {
        if unexpected_failures > 0 {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    } else if counts.failed > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
