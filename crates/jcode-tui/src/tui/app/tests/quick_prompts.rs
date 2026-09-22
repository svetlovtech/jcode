// Fork: quick prompts ([prompts] in config.toml).
//
// `/name` must insert the configured text into the composer for editing,
// never dispatch a turn. Trailing words substitute $ARGUMENTS and $1..$9.
// Unrelated slash commands and unknown names must remain untouched.
//
// Config state is process-global (JCODE_HOME + the config cache), so all
// tests here serialize on the shared env lock and pin the config per test.

use std::sync::MutexGuard;

fn quick_prompts_lock() -> MutexGuard<'static, ()> {
    crate::storage::lock_test_env()
}

/// Point JCODE_HOME at a temp dir holding `config_toml` and force a reload.
/// The caller must hold the env lock for the whole test.
fn quick_prompts_env(config_toml: &str) -> (tempfile::TempDir, Option<std::ffi::OsString>) {
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    let config_path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config parent");
    std::fs::write(&config_path, config_toml).expect("write config");
    crate::config::Config::invalidate_cache();
    (temp, prev_home)
}

fn restore_quick_prompts_env(temp: tempfile::TempDir, prev_home: Option<std::ffi::OsString>) {
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    crate::config::Config::invalidate_cache();
    drop(temp);
}

#[test]
fn quick_prompt_command_inserts_text_into_composer() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) =
        quick_prompts_env("[prompts]\nreview = \"Please review the diff carefully.\"\n");
    let mut app = create_test_app();

    app.input = "/review".to_string();
    app.cursor_pos = app.input.len();
    let handled = super::commands_dispatch::dispatch_local_command(&mut app, "/review");

    assert!(handled, "configured quick prompt should claim the input");
    assert_eq!(app.input, "Please review the diff carefully.");
    assert_eq!(app.cursor_pos, app.input.len());
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn quick_prompt_trailing_words_substitute_arguments() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) =
        quick_prompts_env("[prompts]\nfix = \"Fix $ARGUMENTS in the affected module.\"\n");
    let mut app = create_test_app();

    let handled =
        super::commands_dispatch::dispatch_local_command(&mut app, "/fix the login flow");
    assert!(handled);
    assert_eq!(app.input, "Fix the login flow in the affected module.");
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn quick_prompt_positional_args_substitute() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) =
        quick_prompts_env("[prompts]\ncompare = \"$1 vs $2 - which is better?\"\n");
    let mut app = create_test_app();

    let handled =
        super::commands_dispatch::dispatch_local_command(&mut app, "/compare redis sqlite");
    assert!(handled);
    assert_eq!(app.input, "redis vs sqlite - which is better?");
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn unknown_slash_name_is_not_claimed_by_quick_prompts() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("[prompts]\nreview = \"Review text.\"\n");
    let mut app = create_test_app();

    let handled =
        super::commands_dispatch::dispatch_local_command(&mut app, "/notaprompt");
    assert!(!handled, "unknown names must fall through to other handlers");
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn quick_prompt_candidates_appear_in_the_palette() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("[prompts]\nreview = \"Review the diff.\"\n");
    let app = create_test_app();

    let candidates = app.get_suggestions_for("/");
    assert!(
        candidates.iter().any(|(cmd, _)| cmd == "/review"),
        "configured prompt should be listed in the slash palette; got {candidates:?}"
    );
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn quick_prompts_surface_at_the_front_of_the_bare_slash_palette() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) =
        quick_prompts_env("[prompts]\ndiff-review = \"Review it.\"\nsum = \"Summarize.\"\n");
    let app = create_test_app();

    // A bare `/` prefix-matches ~60 commands that sort by length; without
    // promotion the 8-row window hid everything but the shortest prompts.
    let candidates = app.get_suggestions_for("/");
    let limit = crate::tui::app::COMMAND_SUGGESTION_VISIBLE_LIMIT;
    let visible = &candidates[..candidates.len().min(limit)];
    assert!(
        visible.iter().any(|(cmd, _)| cmd == "/diff-review"),
        "diff-review prompt must be in the visible window; got {visible:?}"
    );
    assert!(
        visible.iter().any(|(cmd, _)| cmd == "/sum"),
        "sum prompt must be in the visible window; got {visible:?}"
    );
    // All matching prompts come before any non-prompt command.
    let first_prompt = candidates.iter().position(|(cmd, _)| cmd == "/diff-review");
    let first_command = candidates
        .iter()
        .position(|(cmd, _)| cmd == "/clear" || cmd == "/model");
    if let (Some(p), Some(c)) = (first_prompt, first_command) {
        assert!(p < c, "prompts must rank before generic commands");
    }

    // A specific command keeps winning its own query: an unmatched prompt
    // must not be dragged into its suggestions.
    let specific = app.get_suggestions_for("/clear");
    assert!(
        !specific.iter().any(|(cmd, _)| cmd == "/diff-review"),
        "typing /clear must not surface unrelated prompts; got {specific:?}"
    );
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn quick_prompt_name_over_64_chars_is_rejected() {
    let _lock = quick_prompts_lock();
    let long_name = "a".repeat(65);
    let config = format!("[prompts]\n{long_name} = \"text\"\n");
    let (temp, prev_home) = quick_prompts_env(&config);
    let mut app = create_test_app();

    let handled = super::commands_dispatch::dispatch_local_command(&mut app, &format!("/{long_name}"));
    assert!(!handled, "over-long prompt names must be ignored by valid_entries");

    restore_quick_prompts_env(temp, prev_home);
}

// File-based prompts: one prompt per file in the [prompts] dir (default
// ~/.jcode/prompts). The file stem is the prompt name.

#[test]
fn prompt_file_is_loaded_and_inserted_into_composer() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("");
    let prompts_dir = temp.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts dir");
    std::fs::write(prompts_dir.join("sum.md"), "Кратко: что сделано и что осталось.\n")
        .expect("write prompt file");

    let mut app = create_test_app();
    let handled = super::commands_dispatch::dispatch_local_command(&mut app, "/sum");

    assert!(handled, "prompt file should claim the input");
    assert_eq!(app.input, "Кратко: что сделано и что осталось.");
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn prompt_file_appears_in_the_palette() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("");
    let prompts_dir = temp.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts dir");
    std::fs::write(prompts_dir.join("review.md"), "Review the diff.").expect("write prompt file");

    let app = create_test_app();
    let candidates = app.get_suggestions_for("/");
    assert!(
        candidates.iter().any(|(cmd, _)| cmd == "/review"),
        "prompt file should be listed in the slash palette; got {candidates:?}"
    );
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn prompt_file_edits_are_visible_without_restart() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("");
    let prompts_dir = temp.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts dir");
    let path = prompts_dir.join("sum.md");
    std::fs::write(&path, "First version.").expect("write prompt file");

    let mut app = create_test_app();
    let handled = super::commands_dispatch::dispatch_local_command(&mut app, "/sum");
    assert!(handled);
    assert_eq!(app.input, "First version.");

    // Rewrite the file and re-dispatch in the same app: file prompts are read
    // fresh from disk on every lookup, no restart or config reload needed.
    std::fs::write(&path, "Second version.").expect("rewrite prompt file");
    app.input = "/sum".to_string();
    app.cursor_pos = app.input.len();
    let handled = super::commands_dispatch::dispatch_local_command(&mut app, "/sum");
    assert!(handled);
    assert_eq!(app.input, "Second version.");
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn prompt_dir_config_points_at_custom_directory() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("[prompts]\ndir = \"myprompts\"\n");
    let prompts_dir = temp.path().join("myprompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts dir");
    std::fs::write(prompts_dir.join("custom.md"), "Custom dir prompt.").expect("write prompt file");

    let mut app = create_test_app();
    let handled = super::commands_dispatch::dispatch_local_command(&mut app, "/custom");
    assert!(handled, "relative [prompts] dir resolves against the jcode home");
    assert_eq!(app.input, "Custom dir prompt.");
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn prompt_dir_config_absolute_path_is_respected() {
    let _lock = quick_prompts_lock();
    let abs_dir = tempfile::tempdir().expect("abs dir");
    std::fs::write(abs_dir.path().join("elsewhere.txt"), "From an absolute dir.")
        .expect("write prompt file");
    let config = format!("[prompts]\ndir = \"{}\"\n", abs_dir.path().display());
    let (temp, prev_home) = quick_prompts_env(&config);
    let mut app = create_test_app();

    let handled = super::commands_dispatch::dispatch_local_command(&mut app, "/elsewhere");
    assert!(handled, "absolute [prompts] dir should be used verbatim");
    assert_eq!(app.input, "From an absolute dir.");
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn inline_prompt_wins_over_same_named_prompt_file() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("[prompts]\nsum = \"Inline wins.\"\n");
    let prompts_dir = temp.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts dir");
    std::fs::write(prompts_dir.join("sum.md"), "File loses.").expect("write prompt file");

    let mut app = create_test_app();
    let handled = super::commands_dispatch::dispatch_local_command(&mut app, "/sum");
    assert!(handled);
    assert_eq!(app.input, "Inline wins.");
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn prompt_files_with_bad_names_or_empty_content_are_skipped() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("");
    let prompts_dir = temp.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir.join("nested")).expect("create prompts dir");
    std::fs::write(prompts_dir.join(".md"), "No stem.").expect("write prompt file");
    std::fs::write(prompts_dir.join("empty.md"), "   \n").expect("write empty prompt file");
    std::fs::write(prompts_dir.join("ignored.json"), "{}").expect("write non-prompt file");
    std::fs::write(prompts_dir.join("nested").join("deep.md"), "Nested prompts are ignored.")
        .expect("write nested prompt file");

    let app = create_test_app();
    let candidates = app.get_suggestions_for("/");
    for bad in ["/empty", "/ignored", "/deep"] {
        assert!(
            !candidates.iter().any(|(cmd, _)| cmd == bad),
            "{bad} must not appear in the palette; got {candidates:?}"
        );
    }
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn missing_prompts_dir_is_harmless() {
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("");
    let app = create_test_app();

    let candidates = app.get_suggestions_for("/");
    assert!(!candidates.iter().any(|(cmd, _)| cmd.starts_with("/sum")));
    restore_quick_prompts_env(temp, prev_home);
}

// Regression: a [prompts] entry added to config.toml while the session runs
// used to stay invisible in the / palette until a restart, because the
// palette candidate cache was never invalidated on config reload (the /name
// expansion read config() directly and worked, which made it more confusing).
// The config-reload hook must drop the cache so the very next keystroke sees
// the new prompt.
#[test]
fn palette_shows_prompt_added_to_config_while_running() {
    use crossterm::event::{KeyCode, KeyEvent};

    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("[prompts]\nsum = \"Summarize.\"\n");
    let mut app = create_test_app();
    assert!(
        !app.get_suggestions_for("/").iter().any(|(cmd, _)| cmd == "/late"),
        "precondition: the prompt is not configured yet"
    );

    // Add a new prompt on disk, then wait past the 500ms config throttle so
    // the next key press re-stats the file (same modeling as
    // keybinding_edit_applies_to_the_next_key_press).
    let config_path = crate::config::Config::path().expect("config path");
    std::fs::write(&config_path, "[prompts]\nsum = \"Summarize.\"\nlate = \"Late entry.\"\n")
        .expect("rewrite config");
    std::thread::sleep(std::time::Duration::from_millis(600));

    // A no-op key press runs the per-keystroke config-reload hook.
    app.handle_key_press_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
        .expect("handle key press");

    let candidates = app.get_suggestions_for("/");
    assert!(
        candidates.iter().any(|(cmd, _)| cmd == "/late"),
        "prompt added to config.toml must appear in the palette without a restart; got {candidates:?}"
    );
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn palette_hint_refreshes_when_prompt_file_is_edited() {
    // The candidates cache fingerprints the prompt files' mtimes; rewriting a
    // prompt file (no config.toml change) must update the palette hint text,
    // not just what /name inserts.
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("");
    let prompts_dir = temp.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts dir");
    let path = prompts_dir.join("sum.md");
    std::fs::write(&path, "First hint line.").expect("write prompt file");

    let app = create_test_app();
    let candidates = app.get_suggestions_for("/");
    let (_, hint) = candidates
        .iter()
        .find(|(cmd, _)| cmd == "/sum")
        .expect("initial prompt listed");
    assert!(
        hint.contains("First hint line."),
        "initial hint should reflect the file; got {hint}"
    );

    // Rewrite the file. The cache fingerprint hashes file mtimes, so the very
    // next suggestion build must observe the edit (mtime resolution caveat:
    // also change the length so coarse-timestamp filesystems see a diff).
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&path, "Second hint text.")
        .expect("rewrite prompt file");

    let candidates = app.get_suggestions_for("/");
    let (_, hint) = candidates
        .iter()
        .find(|(cmd, _)| cmd == "/sum")
        .expect("prompt still listed after edit");
    assert!(
        hint.contains("Second hint text."),
        "palette hint must refresh after a prompt file edit without restart; got {hint}"
    );
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn palette_tracks_prompt_file_renames() {
    // A rename preserves the file's mtime; the fingerprint must include the
    // NAME too, or the palette keeps serving the old prompt set (observed
    // live: review.md -> diff-review.md stayed invisible after the rename).
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("");
    let prompts_dir = temp.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts dir");
    std::fs::write(prompts_dir.join("old.md"), "Old name text.").expect("write prompt file");

    let app = create_test_app();
    assert!(app.get_suggestions_for("/").iter().any(|(cmd, _)| cmd == "/old"));

    std::fs::rename(prompts_dir.join("old.md"), prompts_dir.join("new.md"))
        .expect("rename prompt file");

    let candidates = app.get_suggestions_for("/");
    assert!(
        !candidates.iter().any(|(cmd, _)| cmd == "/old"),
        "renamed-away prompt must disappear; got {candidates:?}"
    );
    assert!(
        candidates.iter().any(|(cmd, _)| cmd == "/new"),
        "renamed-to prompt must appear; got {candidates:?}"
    );
    restore_quick_prompts_env(temp, prev_home);
}

#[test]
fn prompt_file_supports_argument_substitution() {
    // quickfix.md on disk carries $ARGUMENTS; a file prompt must substitute
    // trailing words exactly like an inline [prompts] entry does.
    let _lock = quick_prompts_lock();
    let (temp, prev_home) = quick_prompts_env("");
    let prompts_dir = temp.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts dir");
    std::fs::write(prompts_dir.join("fix.md"), "Fix $ARGUMENTS in the affected module.")
        .expect("write prompt file");

    let mut app = create_test_app();
    let handled =
        super::commands_dispatch::dispatch_local_command(&mut app, "/fix the login timeout");
    assert!(handled);
    assert_eq!(app.input, "Fix the login timeout in the affected module.");
    restore_quick_prompts_env(temp, prev_home);
}
