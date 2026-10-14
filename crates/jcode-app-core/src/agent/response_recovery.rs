use super::tool_markup_recovery::{
    MarkupScan, scan_tool_call_markup, unparsed_markup_notice, unparsed_markup_reminder,
};
use super::*;

/// Outcome of [`Agent::recover_text_wrapped_tool_call`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TextToolRecovery {
    /// Nothing tool-call shaped in the text.
    None,
    /// One or more tool calls were recovered and appended.
    Recovered,
    /// The text holds a tool-call envelope that could not be parsed.
    Unparseable { reason: String },
}

/// A tool call that failed schema validation, kept so the correction and
/// terminal error can repeat exactly what the model did wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MalformedToolCallInfo {
    /// Tool name exactly as the model sent it.
    pub(crate) name: String,
    /// The validation error that was returned to the model as the tool result.
    pub(crate) error: String,
}

impl Agent {
    fn parse_text_wrapped_tool_call(
        text: &str,
    ) -> Option<(String, String, serde_json::Value, String)> {
        let marker = "to=functions.";
        let marker_idx = text.find(marker)?;
        let after_marker = &text[marker_idx + marker.len()..];

        let mut tool_name_end = 0usize;
        for (idx, ch) in after_marker.char_indices() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                tool_name_end = idx + ch.len_utf8();
            } else {
                break;
            }
        }
        if tool_name_end == 0 {
            return None;
        }

        let tool_name = after_marker[..tool_name_end].to_string();
        let remaining = &after_marker[tool_name_end..];
        let mut fallback: Option<(String, String, serde_json::Value, String)> = None;

        for (brace_idx, ch) in remaining.char_indices() {
            if ch != '{' {
                continue;
            }
            let slice = &remaining[brace_idx..];
            let mut stream =
                serde_json::Deserializer::from_str(slice).into_iter::<serde_json::Value>();
            let parsed = match stream.next() {
                Some(Ok(value)) => value,
                Some(Err(_)) | None => continue,
            };
            let consumed = stream.byte_offset();
            if !parsed.is_object() {
                continue;
            }

            let prefix = text[..marker_idx].trim_end().to_string();
            let suffix = remaining[brace_idx + consumed..].trim().to_string();
            if suffix.is_empty() {
                return Some((prefix, tool_name.clone(), parsed, suffix));
            }
            if fallback.is_none() {
                fallback = Some((prefix, tool_name.clone(), parsed, suffix));
            }
        }

        fallback
    }

    /// Recover a tool call the model wrote into its assistant text instead of
    /// emitting it as a structured tool call. Only runs when the response has
    /// no structured tool calls, so structured arguments are never touched.
    ///
    /// Handles the `to=functions.NAME {json}` form and XML/DSML-style
    /// `invoke`/`parameter` envelopes (issue #1702). Recovered calls get ids
    /// starting with `fallback_text_call_`.
    pub(super) fn recover_text_wrapped_tool_call(
        &self,
        text_content: &mut String,
        tool_calls: &mut Vec<ToolCall>,
    ) -> TextToolRecovery {
        Self::recover_text_tool_calls(text_content, tool_calls)
    }

    pub(super) fn recover_text_tool_calls(
        text_content: &mut String,
        tool_calls: &mut Vec<ToolCall>,
    ) -> TextToolRecovery {
        if !tool_calls.is_empty() || text_content.trim().is_empty() {
            return TextToolRecovery::None;
        }

        let Some((prefix, tool_name, arguments, suffix)) =
            Self::parse_text_wrapped_tool_call(text_content)
        else {
            return Self::recover_markup_tool_calls(text_content, tool_calls);
        };

        let mut sanitized = String::new();
        if !prefix.is_empty() {
            sanitized.push_str(&prefix);
        }
        if !suffix.is_empty() {
            if !sanitized.is_empty() {
                sanitized.push('\n');
            }
            sanitized.push_str(&suffix);
        }
        *text_content = sanitized;

        let call_id = format!("fallback_text_call_{}", id::new_id("call"));
        let recovered_total = RECOVERED_TEXT_WRAPPED_TOOL_CALLS
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        logging::warn(&format!(
            "[agent] Recovered text-wrapped tool call for '{}' ({}, total={})",
            tool_name, call_id, recovered_total
        ));
        let intent = ToolCall::intent_from_input(&arguments);
        tool_calls.push(ToolCall {
            id: call_id,
            name: tool_name,
            input: arguments,
            intent,
            thought_signature: None,
        });

        TextToolRecovery::Recovered
    }

    fn recover_markup_tool_calls(
        text_content: &mut String,
        tool_calls: &mut Vec<ToolCall>,
    ) -> TextToolRecovery {
        match scan_tool_call_markup(text_content) {
            MarkupScan::None => TextToolRecovery::None,
            MarkupScan::Unparseable { reason } => {
                logging::warn(&format!(
                    "[agent] Assistant text contains an unparseable tool-call markup envelope ({reason})"
                ));
                logging::debug(&format!(
                    "[agent] Unparsed tool-call markup envelope (raw): {}",
                    text_content
                ));
                TextToolRecovery::Unparseable { reason }
            }
            MarkupScan::Parsed {
                calls,
                sanitized_text,
            } => {
                *text_content = sanitized_text;
                for call in calls {
                    let call_id = format!("fallback_text_call_{}", id::new_id("call"));
                    let recovered_total = RECOVERED_TEXT_WRAPPED_TOOL_CALLS
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                        + 1;
                    logging::warn(&format!(
                        "[agent] Recovered tool-call markup envelope for '{}' ({}, total={})",
                        call.name, call_id, recovered_total
                    ));
                    let intent = ToolCall::intent_from_input(&call.arguments);
                    tool_calls.push(ToolCall {
                        id: call_id,
                        name: call.name,
                        input: call.arguments,
                        intent,
                        thought_signature: None,
                    });
                }
                TextToolRecovery::Recovered
            }
        }
    }

    /// Cap on recovery nudges per turn for unparseable tool-call markup.
    pub(crate) const MAX_UNPARSED_TOOL_MARKUP_NUDGES: u32 = 2;

    /// When the response ended with an unparseable tool-call envelope as its
    /// only output, ask the model to re-issue the call instead of silently
    /// ending the turn (issue #1702). Returns the user-visible notice when a
    /// nudge was injected and the caller should re-issue the request.
    pub(crate) fn maybe_nudge_unparsed_tool_markup(
        &mut self,
        recovery: &TextToolRecovery,
        stop_reason: Option<&str>,
        attempts: &mut u32,
    ) -> Result<Option<String>> {
        let TextToolRecovery::Unparseable { reason } = recovery else {
            return Ok(None);
        };
        // A truncated response (max_tokens etc.) is handled by the
        // incomplete-response continuation, which resumes mid-output.
        if stop_reason
            .map(str::trim)
            .is_some_and(Self::should_continue_after_stop_reason)
        {
            return Ok(None);
        }
        if *attempts >= Self::MAX_UNPARSED_TOOL_MARKUP_NUDGES {
            logging::warn(&format!(
                "Unparseable tool-call markup persisted after {} nudges; ending turn",
                attempts
            ));
            return Ok(None);
        }
        *attempts += 1;
        logging::warn(&format!(
            "Requesting re-issue of unparseable tool-call markup (attempt {}/{})",
            attempts,
            Self::MAX_UNPARSED_TOOL_MARKUP_NUDGES
        ));
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: unparsed_markup_reminder(reason),
                cache_control: None,
            }],
        );
        self.session.save()?;
        Ok(Some(unparsed_markup_notice(reason)))
    }

    pub(crate) fn should_continue_after_stop_reason(stop_reason: &str) -> bool {
        let reason = stop_reason.trim().to_ascii_lowercase();
        if reason.is_empty() {
            return false;
        }

        if matches!(reason.as_str(), "stop" | "end_turn" | "tool_use") {
            return false;
        }

        reason.contains("incomplete")
            || reason.contains("max_output_tokens")
            || reason.contains("max_tokens")
            || reason.contains("length")
            || reason.contains("trunc")
            || reason.contains("commentary")
    }

    /// True when the provider's stop reason indicates a model-side
    /// guardrail/safety stop (e.g. Anthropic `refusal`), as opposed to a
    /// normal end-of-turn or truncation.
    pub(crate) fn is_guardrail_stop_reason(stop_reason: Option<&str>) -> bool {
        let Some(reason) = stop_reason else {
            return false;
        };
        let reason = reason.trim().to_ascii_lowercase();
        matches!(reason.as_str(), "refusal" | "content_filter" | "safety")
            || reason.contains("guardrail")
            || reason.contains("policy_violation")
    }

    pub(crate) const FABLE_GUARDRAIL_RECONSIDERATION_PROMPTS: [&'static str; 3] = [
        "Reassess the user's request using its full context and likely intent rather than isolated keywords. Determine whether answering would enable a concrete harmful action. If no specific safety concern applies, answer normally. Preserve the refusal only when it is genuinely warranted, and briefly identify the specific concern.",
        "Review the request again and distinguish legitimate coding, debugging, defensive, educational, or analytical work from instructions that facilitate harm. If only part of the request is unsafe, help with the safe portions and narrowly limit the rest instead of issuing a blanket refusal.",
        "Perform one final, independent policy check. Prefer a safe and useful constrained response when possible. Refuse only the specific content that creates a concrete safety risk; otherwise continue with the user's actual task. Do not weaken a refusal that remains genuinely necessary.",
    ];

    /// Try a small sequence of differently framed policy checks after Fable
    /// guardrails a response. Every prompt preserves warranted refusals, and the
    /// fixed suite size prevents an unbounded refusal/retry loop.
    pub(crate) fn maybe_reconsider_fable_guardrail(
        &mut self,
        stop_reason: Option<&str>,
        attempts: &mut u32,
    ) -> Result<bool> {
        let model = self.provider.model();
        if !Self::should_reconsider_fable_guardrail(
            &model,
            stop_reason,
            *attempts,
            Self::FABLE_GUARDRAIL_RECONSIDERATION_PROMPTS.len() as u32,
        ) {
            return Ok(false);
        }

        let prompt = Self::FABLE_GUARDRAIL_RECONSIDERATION_PROMPTS[*attempts as usize];
        *attempts += 1;
        logging::warn(&format!(
            "Fable 5 guardrail stopped the response (stop_reason={:?}); trying reconsideration prompt {}/{}",
            stop_reason,
            attempts,
            Self::FABLE_GUARDRAIL_RECONSIDERATION_PROMPTS.len(),
        ));
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: prompt.to_string(),
                cache_control: None,
            }],
        );
        self.session.save()?;
        Ok(true)
    }

    pub(crate) fn should_reconsider_fable_guardrail(
        model: &str,
        stop_reason: Option<&str>,
        attempts: u32,
        max_attempts: u32,
    ) -> bool {
        Self::is_guardrail_stop_reason(stop_reason)
            && model.to_ascii_lowercase().contains("fable-5")
            && attempts < max_attempts
    }

    /// Builds the user-facing notice for a turn that ended with no visible
    /// assistant output (no text, no tool calls). Returns `None` when the turn
    /// looks normal and no notice should be surfaced.
    pub(crate) fn provider_guardrail_notice(
        stop_reason: Option<&str>,
        visible_text_empty: bool,
        had_reasoning: bool,
    ) -> Option<String> {
        let guardrail = Self::is_guardrail_stop_reason(stop_reason);
        if !guardrail && !visible_text_empty {
            return None;
        }
        let reason_label = stop_reason
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .unwrap_or("unknown");
        if guardrail {
            return Some(format!(
                "Provider guardrail stopped the response (stop_reason: {}). The model declined to answer this request. Rephrasing, narrowing the request, or providing more context may help.",
                reason_label
            ));
        }
        // Empty visible output with a non-guardrail stop reason: still surface,
        // since the user otherwise sees nothing at all. Do not assert a content
        // filter here: in practice this is usually a transient upstream failure
        // (a dropped or empty stream), not a provider guardrail (issue #672).
        let reasoning_hint = if had_reasoning {
            " after producing only internal reasoning"
        } else {
            ""
        };
        Some(format!(
            "The model ended its turn without any visible output{} (stop_reason: {}). The provider returned an empty response; this is usually a transient upstream failure rather than a content filter. Retrying the request may help.",
            reasoning_hint, reason_label
        ))
    }

    /// Log-event label for an empty final turn: real guardrail stops keep the
    /// `PROVIDER_GUARDRAIL` name, transient empty responses get their own so
    /// the two are separable in logs (issue #672).
    pub(crate) fn empty_turn_log_event(stop_reason: Option<&str>) -> &'static str {
        if Self::is_guardrail_stop_reason(stop_reason) {
            "PROVIDER_GUARDRAIL"
        } else {
            "PROVIDER_EMPTY_RESPONSE"
        }
    }

    /// Retry a whitespace-only final response that arrived right after tool
    /// results, by asking the model to produce the final answer. Shared by the
    /// non-streaming and streaming (mpsc) turn loops so their recovery
    /// behavior cannot drift (issue #672). Returns true when a continuation
    /// message was injected and the caller should re-issue the request.
    pub(crate) fn maybe_continue_empty_post_tool_response(
        &mut self,
        visible_text_empty: bool,
        prompt_has_recent_tool_result: bool,
        stop_reason: Option<&str>,
        attempts: &mut u32,
    ) -> Result<bool> {
        if !visible_text_empty || !prompt_has_recent_tool_result {
            return Ok(false);
        }
        // A model-side refusal is deliberate; retrying it just burns tokens.
        if Self::is_guardrail_stop_reason(stop_reason) {
            return Ok(false);
        }
        if *attempts >= Self::MAX_EMPTY_POST_TOOL_CONTINUATION_ATTEMPTS {
            return Ok(false);
        }
        *attempts += 1;
        logging::warn(&format!(
            "Provider returned whitespace-only final response after tool results (stop_reason={:?}); requesting final answer continuation (attempt {}/{})",
            stop_reason,
            attempts,
            Self::MAX_EMPTY_POST_TOOL_CONTINUATION_ATTEMPTS
        ));
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                // Keep this as a user-role message for provider compatibility,
                // but mark it as internal so transcript renderers never present
                // the synthetic recovery instruction as a prompt from the user.
                text: "<system-reminder>The previous provider response was empty after tool results. Provide the final answer to the user's last request using the tool results above. Do not call more tools unless absolutely necessary.</system-reminder>".to_string(),
                cache_control: None,
            }],
        );
        self.session.save()?;
        Ok(true)
    }

    fn continuation_prompt_for_stop_reason(stop_reason: &str) -> String {
        format!(
            "[System reminder: your previous response ended before completion (stop_reason: {}). Continue exactly where you left off, do not repeat completed content, and if the next step is a tool call, emit the tool call now.]",
            stop_reason.trim()
        )
    }

    pub(crate) fn maybe_continue_incomplete_response(
        &mut self,
        stop_reason: Option<&str>,
        attempts: &mut u32,
    ) -> Result<bool> {
        let Some(stop_reason) = stop_reason
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
        else {
            return Ok(false);
        };

        if !Self::should_continue_after_stop_reason(stop_reason) {
            return Ok(false);
        }

        if *attempts >= Self::MAX_INCOMPLETE_CONTINUATION_ATTEMPTS {
            logging::warn(&format!(
                "Response ended with stop_reason='{}' after {} continuation attempts; returning partial output",
                stop_reason, attempts
            ));
            return Ok(false);
        }

        *attempts += 1;
        logging::warn(&format!(
            "Response ended with stop_reason='{}'; requesting continuation (attempt {}/{})",
            stop_reason,
            attempts,
            Self::MAX_INCOMPLETE_CONTINUATION_ATTEMPTS
        ));

        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: Self::continuation_prompt_for_stop_reason(stop_reason),
                cache_control: None,
            }],
        );
        self.session.save()?;
        Ok(true)
    }

    /// True when the provider said it stopped to call a tool but no tool call
    /// survived parsing.
    ///
    /// `stop_reason: tool_use` with zero tool calls is a contradiction: the
    /// model intended to act and the harness has nothing to run. Breaking out
    /// of the turn there strands the agent mid-task, which on a benchmark run
    /// looks like an ordinary "the agent stopped early" failure and silently
    /// discards all of its uncommitted work. Treat it like any other
    /// incomplete response and ask for a continuation instead.
    pub(crate) fn is_stranded_tool_use_stop(stop_reason: Option<&str>) -> bool {
        stop_reason
            .map(str::trim)
            .map(|reason| reason.eq_ignore_ascii_case("tool_use"))
            .unwrap_or(false)
    }

    pub(crate) fn maybe_continue_stranded_tool_use(
        &mut self,
        stop_reason: Option<&str>,
        attempts: &mut u32,
    ) -> Result<bool> {
        if !Self::is_stranded_tool_use_stop(stop_reason) {
            return Ok(false);
        }
        if *attempts >= Self::MAX_INCOMPLETE_CONTINUATION_ATTEMPTS {
            logging::warn(&format!(
                "Provider reported stop_reason='tool_use' with no parsed tool call after {} continuation attempts; ending turn",
                attempts
            ));
            return Ok(false);
        }
        *attempts += 1;
        logging::warn(&format!(
            "Provider reported stop_reason='tool_use' but no tool call was parsed; requesting continuation (attempt {}/{})",
            attempts,
            Self::MAX_INCOMPLETE_CONTINUATION_ATTEMPTS
        ));
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "[System reminder: your previous response ended with stop_reason \"tool_use\" but no tool call arrived. Nothing was executed. Re-issue the tool call you intended, do not repeat completed work, and continue the task.]"
                    .to_string(),
                cache_control: None,
            }],
        );
        self.session.save()?;
        Ok(true)
    }

    pub(super) fn filter_truncated_tool_calls(
        &mut self,
        stop_reason: Option<&str>,
        tool_calls: &mut Vec<ToolCall>,
        assistant_message_id: Option<&String>,
    ) {
        let stop_reason = stop_reason.unwrap_or("");
        if !Self::should_continue_after_stop_reason(stop_reason) {
            return;
        }

        let before = tool_calls.len();
        tool_calls.retain(|tc| !tc.input.is_null());
        let discarded = before - tool_calls.len();
        if discarded > 0 && tool_calls.is_empty() {
            logging::warn(&format!(
                "Discarded {} tool call(s) with null input (truncated by {}); requesting continuation",
                discarded,
                if stop_reason.is_empty() {
                    "unknown"
                } else {
                    stop_reason
                }
            ));
            if let Some(msg_id) = assistant_message_id {
                self.session.remove_tool_use_blocks(msg_id);
                self.persist_session_best_effort("truncated tool-call repair");
            }
        }
    }
}
