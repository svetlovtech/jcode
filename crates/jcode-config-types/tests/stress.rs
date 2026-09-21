use jcode_config_types::QuickPromptsConfig;
use std::sync::Arc;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn valid_entries_is_shareable_across_threads() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    // SAFETY: process-global test configuration; see file_prompts.rs.
    unsafe { std::env::set_var("JCODE_HOME", temp.path()) };
    let cfg = Arc::new(QuickPromptsConfig::default());
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let cfg = Arc::clone(&cfg);
            std::thread::spawn(move || cfg.valid_entries().is_empty())
        })
        .collect();
    let all_empty = handles.into_iter().all(|h| h.join().unwrap());
    // SAFETY: as above.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("JCODE_HOME", v),
            None => std::env::remove_var("JCODE_HOME"),
        }
    }
    assert!(all_empty);
}
