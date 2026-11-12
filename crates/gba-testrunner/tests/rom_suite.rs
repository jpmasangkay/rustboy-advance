use gba_testrunner::{
    RunOptions, TestOutcome, discover_workspace_root, load_manifest, run_suite, summarize_results,
};

#[test]
fn run_manifest_suite_skips_missing_roms() {
    let root = discover_workspace_root();
    let manifest = load_manifest(None, &root).expect("load default manifest");
    let opts = RunOptions::for_workspace(&root);
    let results = run_suite(&root, &manifest, &opts).expect("execute test suite");
    assert_eq!(results.len(), manifest.tests.len());

    let counts = summarize_results(&results);
    let mut unexpected_failures = Vec::new();
    for res in &results {
        if res.outcome == TestOutcome::Fail && !res.known_failure {
            unexpected_failures.push(format!("{} ({}): {}", res.id, res.name, res.notes));
        }
    }

    eprintln!(
        "gba-testrunner summary: {} passed, {} failed, {} skipped (missing ROM), {} skipped (missing golden)",
        counts.passed, counts.failed, counts.skipped_rom, counts.skipped_golden
    );

    assert!(
        unexpected_failures.is_empty(),
        "Unexpected test ROM failures:\n{}",
        unexpected_failures.join("\n")
    );
}
