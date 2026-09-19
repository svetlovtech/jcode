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
