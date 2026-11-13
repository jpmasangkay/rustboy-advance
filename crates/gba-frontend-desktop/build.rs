use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn format_utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;

    // Howard Hinnant's civil-from-days algorithm (UTC).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn read_git_commit() -> String {
    let rev = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output();
    let Ok(out) = rev else {
        return "unknown".to_string();
    };
    if !out.status.success() {
        return "unknown".to_string();
    }
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if hash.is_empty() {
        return "unknown".to_string();
    }
    let dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .map(|s| s.status.success() && !String::from_utf8_lossy(&s.stdout).trim().is_empty())
        .unwrap_or(false);
    if dirty {
        format!("{hash}-dirty")
    } else {
        hash
    }
}

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    let git_head = Path::new("../../.git/HEAD");
    if git_head.exists() {
        println!("cargo:rerun-if-changed=../../.git/HEAD");
    }
    println!("cargo:rustc-env=RBA_GIT_COMMIT={}", read_git_commit());
    println!("cargo:rustc-env=RBA_BUILD_TIME_UTC={}", format_utc_now());
}

