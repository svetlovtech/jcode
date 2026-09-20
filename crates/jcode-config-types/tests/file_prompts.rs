use jcode_config_types::QuickPromptsConfig;

/// JCODE_HOME is process-global; the three tests must not race on it.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Environment-mode test: runs in its own process with a temp JCODE_HOME so
/// the directory resolution (the real code path used by the TUI) is exercised
/// end to end, including extension filtering and name validation.
fn setup_home(files: &[(&str, &str)]) -> (tempfile::TempDir, Option<std::ffi::OsString>) {
    let temp = tempfile::tempdir().expect("tempdir");
    let prev = std::env::var_os("JCODE_HOME");
    // SAFETY: single-threaded env-mutation test, matching the centralized
    // jcode-core::env pattern (env mutation is process-global configuration).
    unsafe { std::env::set_var("JCODE_HOME", temp.path()) };
    for (name, content) in files {
        let path = temp.path().join("prompts").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    (temp, prev)
}

fn restore(temp: tempfile::TempDir, prev: Option<std::ffi::OsString>) {
    // SAFETY: see setup_home.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("JCODE_HOME", v),
            None => std::env::remove_var("JCODE_HOME"),
        }
    }
    drop(temp);
}

#[test]
fn prompt_files_load_through_the_default_dir() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (temp, prev) = setup_home(&[
        ("sum.md", "Кратко: итог.\n"),
        ("review.txt", "Review it.\n"),
        ("UPPER.MD", "Upper case extension.\n"),
        ("crlf.md", "Line one\r\nLine two\r\n"),
        ("skipme.json", "{}"),
        ("empty.md", "  \n\t\n"),
    ]);
    let cfg = QuickPromptsConfig::default();
    let names: Vec<String> = cfg.valid_entries().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, vec!["UPPER", "crlf", "review", "sum"]);
    let entries = cfg.valid_entries();
    let sum = entries.iter().find(|(n, _)| n == "sum").unwrap();
    assert_eq!(sum.1, "Кратко: итог.");
    let crlf = entries.iter().find(|(n, _)| n == "crlf").unwrap();
    assert_eq!(crlf.1, "Line one\r\nLine two");
    restore(temp, prev);
}

#[test]
fn inline_config_key_wins_and_dir_may_be_overridden() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (temp, prev) = setup_home(&[("sum.md", "From file.")]);
    // Parse the [prompts] section exactly like Config does: the section
    // content becomes the QuickPromptsConfig table.
    let raw = "[prompts]\nsum = \"Inline wins.\"\n";
    let doc: toml::Value = toml::from_str(raw).unwrap();
    let cfg: QuickPromptsConfig = doc.get("prompts").unwrap().clone().try_into().unwrap();
    let entries = cfg.valid_entries();
    assert_eq!(entries, vec![("sum".to_string(), "Inline wins.".to_string())]);

    let abs = tempfile::tempdir().unwrap();
    std::fs::write(abs.path().join("elsewhere.md"), "Absolute dir content.").unwrap();
    let raw = format!("[prompts]\ndir = \"{}\"\n", abs.path().display());
    let doc: toml::Value = toml::from_str(&raw).unwrap();
    let cfg: QuickPromptsConfig = doc.get("prompts").unwrap().clone().try_into().unwrap();
    let entries = cfg.valid_entries();
    assert_eq!(
        entries,
        vec![("elsewhere".to_string(), "Absolute dir content.".to_string())]
    );
    restore(temp, prev);
}

#[test]
fn missing_and_unreadable_prompt_files_degrade_gracefully() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (temp, prev) = setup_home(&[]);
    // No prompts dir at all.
    assert!(QuickPromptsConfig::default().valid_entries().is_empty());

    // A directory named like a prompt file is not a file: skipped.
    std::fs::create_dir_all(temp.path().join("prompts").join("not_a_file.md")).unwrap();
    assert!(QuickPromptsConfig::default().valid_entries().is_empty());
    restore(temp, prev);
}

#[test]
fn unreadable_prompt_file_is_skipped_without_failing_others() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (temp, prev) = setup_home(&[("good.md", "Good prompt.")]);
    let unreadable = temp.path().join("prompts").join("locked.md");
    std::fs::write(&unreadable, "Secret?").unwrap();
    let mut perms = std::fs::metadata(&unreadable).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o000);
    std::fs::set_permissions(&unreadable, perms).expect("chmod 000");

    let entries = QuickPromptsConfig::default().valid_entries();
    let names: Vec<String> = entries.into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, vec!["good"], "unreadable file must be skipped, not fatal");
    restore(temp, prev);
}
