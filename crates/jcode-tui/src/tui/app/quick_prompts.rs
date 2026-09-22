//! Fork: quick prompts (`[prompts]` in config.toml).
//!
//! Owns everything quick-prompt related that lives in the TUI crate:
//!   * `handle_quick_prompt_command` - `/name` expansion into the composer,
//!   * `prompt_sources_fingerprint` - palette cache invalidation,
//!   * `intern_hint` - static hint interning for the palette pipeline.
//!
//! Config types live in `jcode-config-types::QuickPromptsConfig`; the palette
//! entries are built in `state_ui_input_helpers.rs`.

use super::App;

/// Fork: insert a quick prompt's text into the composer.
///
/// `/name` (optionally followed by extra typed text) resolves against
/// `[prompts]` in config.toml. The prompt text replaces the command; any
/// trailing text the user typed after the command name is appended so
/// `/review focus on the auth module` expands to "<review text> focus on
/// the auth module". `$ARGUMENTS` / `$1`-`$9` in the text are substituted
/// from the trailing words when present, otherwise left as-is for editing.
/// Returns `true` when `input` named a configured quick prompt.
pub(crate) fn handle_quick_prompt_command(app: &mut App, input: &str) -> bool {
    let Some(command) = input.strip_prefix('/') else {
        return false;
    };
    let (name, trailing) = match command.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, Some(rest.trim())),
        None => (command, None),
    };
    if name.is_empty() {
        return false;
    }

    let Some((_, text)) = crate::config::config()
        .prompts
        .valid_entries()
        .into_iter()
        .find(|(entry_name, _)| entry_name == name)
    else {
        return false;
    };

    // Trailing words substitute $ARGUMENTS / positional placeholders.
    let args: Vec<&str> = trailing
        .filter(|rest| !rest.is_empty())
        .map(|rest| rest.split_whitespace().collect())
        .unwrap_or_default();
    let expanded = if args.is_empty() {
        text
    } else {
        let mut out = text;
        for (index, arg) in args.iter().enumerate() {
            out = out.replace(&format!("${}", index + 1), arg);
        }
        let joined = args.join(" ");
        if out.contains("$ARGUMENTS") {
            out.replace("$ARGUMENTS", &joined)
        } else {
            out
        }
    };

    // Replace the composer content with the expanded prompt (undo-able), as
    // if the user typed it.
    app.remember_input_undo_state();
    app.input = expanded;
    app.cursor_pos = app.input.len();
    app.reset_tab_completion();
    app.set_status_notice(format!("Quick prompt /{name} inserted - edit and send"));
    true
}

/// Fingerprint the quick-prompt sources so the palette notices prompt-file
/// changes without polling: the config reload generation (catches config.toml
/// edits) hashed with each prompt file's NAME and mtime. Names matter as much
/// as times: a rename preserves the mtime, and without the name in the hash
/// the multiset - and therefore the fingerprint - would not change. The set
/// is tiny, so stat-ing per frame is cheap. A missing dir contributes a
/// stable constant.
pub(crate) fn prompt_sources_fingerprint() -> u64 {
    let config = crate::config::config();
    let mut state: u64 = crate::config::config_reload_generation();
    if let Some(dir) = config.prompts.prompt_dir() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let name_hash: u64 = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| {
                        name.bytes().fold(0xcbf_29ce_4842_2325u64, |acc, byte| {
                            (acc ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
                        })
                    })
                    .unwrap_or(0);
                state ^= name_hash
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .rotate_left(13);
                state = state.rotate_left(7);
                if let Ok(meta) = entry.metadata()
                    && let Ok(modified) = meta.modified()
                    && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
                {
                    let nanos = since.as_nanos() as u64;
                    state ^= nanos
                        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        .rotate_left(17);
                    state = state.rotate_left(7);
                }
            }
        }
    }
    state
}

/// Intern a prompt hint into a `&'static str`, deduplicating by content.
///
/// The suggestion pipeline carries help strings as `&'static str` (most come
/// from `REGISTERED_COMMANDS`, which is truly static). Prompt hints are built
/// from dynamic text, and the naive `Box::leak` per cache rebuild leaked a new
/// copy every time the candidates cache was invalidated. Interning keeps the
/// `&'static str` contract while leaking each distinct hint at most once; the
/// table is bounded by the number of distinct prompt texts a user ever has.
pub(crate) fn intern_hint(hint: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    static INTERNED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let interned = INTERNED.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(set) = interned.lock() {
        if let Some(existing) = set.get(hint) {
            return existing;
        }
    }
    let leaked: &'static str = Box::leak(hint.to_string().into_boxed_str());
    if let Ok(mut set) = interned.lock() {
        set.insert(leaked);
    }
    leaked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_for_identical_sources() {
        let first = prompt_sources_fingerprint();
        let second = prompt_sources_fingerprint();
        assert_eq!(first, second, "same sources must hash identically");
    }

    #[test]
    fn intern_hint_deduplicates_by_content() {
        let a = intern_hint("Insert prompt: review");
        let b = intern_hint("Insert prompt: review");
        assert_eq!(a, b);
        assert!(std::ptr::eq(a, b), "same content must reuse one leak");
        let c = intern_hint("Insert prompt: other");
        assert!(!std::ptr::eq(a, c), "distinct content must not alias");
    }
}
