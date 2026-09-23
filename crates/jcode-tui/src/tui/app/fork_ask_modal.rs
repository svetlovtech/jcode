//! Fork: interactive ask_user modal for the TUI, split into focused
//! submodules: `fork_ask_state` (types + stateful helpers + protocol
//! conversion + tests), `fork_ask_keys` (keyboard handling and answer
//! staging), `fork_ask_render` (drawing). The public API is re-exported so
//! existing `fork_ask_modal::` paths keep resolving.

#[path = "fork_ask_keys.rs"]
mod fork_ask_keys;
#[path = "fork_ask_render.rs"]
mod fork_ask_render;
#[path = "fork_ask_state.rs"]
mod fork_ask_state;

pub(crate) use fork_ask_keys::handle_modal_key;
pub use fork_ask_render::draw_ask_modal;
pub use fork_ask_state::{AskModal, AskSpecUi};
