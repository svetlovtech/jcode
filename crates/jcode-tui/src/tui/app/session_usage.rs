//! Fork: session usage-total lifecycle.
//!
//! Owns the `/clear` semantics for accumulated usage state: the aggregate
//! token counters, the priced dollar cost, cached per-turn stats, and the
//! totals restored from remote history. Both `/clear` paths (local
//! `reset_current_session` in `commands_review.rs` and remote `/clear` in
//! `remote/key_handling.rs`) call [`reset_session_usage_totals`] so they
//! cannot drift apart.

use super::{App, CostState, TokenAccounting};

impl App {
    /// Reset the accumulated session usage totals that survive a turn: the
    /// aggregate token counters, the priced dollar cost, and the cached
    /// per-turn stats. `/clear` discards the whole transcript, so its totals
    /// (the "Σ tokens" footer, the session cost, `/cache` and `/info`) must
    /// restart from zero like everything else.
    pub(super) fn reset_session_usage_totals(&mut self) {
        self.token_accounting = TokenAccounting::default();
        self.cost = CostState::default();
        self.last_turn_input_tokens = None;
        self.last_api_completed = None;
        self.last_api_completed_provider = None;
        self.last_api_completed_model = None;
        // Restored-from-history totals: the new session has no history, so a
        // later History event (e.g. a reload) must re-seed them from scratch.
        self.remote_total_tokens = None;
        self.remote_token_usage_totals = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TokenUsageTotals;
    use std::time::Instant;

    #[test]
    fn reset_session_usage_totals_zeroes_every_accumulator() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.token_accounting.total_input_tokens = 180_000;
        app.token_accounting.total_output_tokens = 9_000;
        app.cost.total_cost = 1.25;
        app.last_turn_input_tokens = Some(95_000);
        app.last_api_completed = Some(Instant::now());
        app.last_api_completed_provider = Some("anthropic".to_string());
        app.last_api_completed_model = Some("claude-test".to_string());
        app.remote_total_tokens = Some((150_000, 8_000));
        app.remote_token_usage_totals = Some(TokenUsageTotals {
            cache_prompt_tokens: Some(120_000),
            messages_with_token_usage: 12,
            input_tokens: 150_000,
            output_tokens: 8_000,
            cache_reported_input_tokens: 150_000,
            cache_read_input_tokens: 100_000,
            cache_creation_input_tokens: 20_000,
        });

        app.reset_session_usage_totals();

        assert_eq!(app.token_accounting.total_input_tokens, 0);
        assert_eq!(app.token_accounting.total_output_tokens, 0);
        assert_eq!(app.cost.total_cost, 0.0);
        assert_eq!(app.last_turn_input_tokens, None);
        assert_eq!(app.last_api_completed, None);
        assert_eq!(app.last_api_completed_provider, None);
        assert_eq!(app.last_api_completed_model, None);
        assert_eq!(app.remote_total_tokens, None);
        assert_eq!(app.remote_token_usage_totals, None);
    }
}
