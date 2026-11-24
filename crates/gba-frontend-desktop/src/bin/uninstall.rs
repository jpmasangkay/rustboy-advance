#![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
const MB_OK: u32 = 0x0000_0000;
#[cfg(windows)]
const MB_YESNO: u32 = 0x0000_0004;
#[cfg(windows)]
const MB_ICONERROR: u32 = 0x0000_0010;
#[cfg(windows)]
const MB_ICONWARNING: u32 = 0x0000_0030;
#[cfg(windows)]
const MB_ICONINFORMATION: u32 = 0x0000_0040;
#[cfg(windows)]
const MB_DEFBUTTON2: u32 = 0x0000_0100;
#[cfg(windows)]
const IDYES: i32 = 6;

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, utype: u32) -> i32;
}

#[cfg(windows)]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn ask_confirmation(title: &str, body: &str) -> bool {
    let wide_text = to_wide_null(body);
    let wide_title = to_wide_null(title);
    let res = unsafe {
        MessageBoxW(
            0,
            wide_text.as_ptr(),
            wide_title.as_ptr(),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    };
    res == IDYES
}

#[cfg(not(windows))]
fn ask_confirmation(title: &str, body: &str) -> bool {
    use std::io::{self, Write};
    eprintln!("{title}\n\n{body}");
    eprint!("Proceed? [y/N]: ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_ok() {
        let trimmed = line.trim();
        return trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes");
    }
    false
}

#[cfg(windows)]
fn show_message(title: &str, body: &str, is_error: bool) {
    let wide_text = to_wide_null(body);
    let wide_title = to_wide_null(title);
    let icon = if is_error {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    unsafe {
        MessageBoxW(0, wide_text.as_ptr(), wide_title.as_ptr(), MB_OK | icon);
    }
}

#[cfg(not(windows))]
fn show_message(title: &str, body: &str, is_error: bool) {
    if is_error {
        eprintln!("{title}: {body}");
    } else {
        println!("{title}: {body}");
    }
}

fn sibling_emulator_exe(current_exe: &Path) -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        "rustboy-advance.exe"
    } else {
        "rustboy-advance"
    };
    current_exe.parent().map(|dir| dir.join(exe_name))
}

fn main() {
    let skip_confirm = std::env::args().any(|a| a == "--yes" || a == "-y");

    let title = "Uninstall Rustboy Advance";
    let confirm_body = "Remove all Rustboy Advance app data, settings, battery saves, save states, covers, screenshots, playtest logs, and saved credentials?\n\nYour ROM files will NOT be deleted.\nAfterward, you can delete this folder to finish uninstalling.";

    if !skip_confirm && !ask_confirmation(title, confirm_body) {
        return;
    }

    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(err) => {
            show_message(
                title,
                &format!("Could not locate the uninstaller executable:\n{err}"),
                true,
            );
            std::process::exit(1);
        }
    };

    let Some(emu_exe) = sibling_emulator_exe(&current_exe) else {
        show_message(
            title,
            "Could not determine the installation folder.",
            true,
        );
        std::process::exit(1);
    };

    if !emu_exe.is_file() {
        show_message(
            title,
            &format!(
                "Could not find {} next to this uninstaller.\nMake sure both files remain in the same folder.",
                emu_exe
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "rustboy-advance.exe".to_string())
            ),
            true,
        );
        std::process::exit(1);
    }

    let mut cmd = Command::new(&emu_exe);
    cmd.args(["--uninstall", "--yes"]);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.output() {
        Ok(output) if output.status.success() => {
            if !skip_confirm {
                show_message(
                    title,
                    "All Rustboy Advance persistent app data, saves, states, covers, screenshots, and credentials have been removed.\n\nYou may now delete this folder to complete uninstallation.",
                    false,
                );
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let detail = if stderr.is_empty() {
                format!("Exit status: {}", output.status)
            } else {
                stderr
            };
            show_message(title, &format!("Uninstall failed:\n{detail}"), true);
            std::process::exit(1);
        }
        Err(err) => {
            show_message(
                title,
                &format!("Failed to launch rustboy-advance uninstaller:\n{err}"),
                true,
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_sibling_emulator_binary() {
        let p = PathBuf::from(if cfg!(windows) {
            r"C:\Games\Rustboy-Advance\uninstall.exe"
        } else {
            "/opt/Rustboy-Advance/uninstall"
        });
        let expected = PathBuf::from(if cfg!(windows) {
            r"C:\Games\Rustboy-Advance\rustboy-advance.exe"
        } else {
            "/opt/Rustboy-Advance/rustboy-advance"
        });
        assert_eq!(sibling_emulator_exe(&p), Some(expected));
    }

    #[cfg(windows)]
    #[test]
    fn wide_string_is_null_terminated() {
        let w = to_wide_null("OK");
        assert_eq!(w, vec!['O' as u16, 'K' as u16, 0]);
    }
}
