//! Application-local services; core domain models live in hunkra-core.
#[cfg(feature = "gui")]
pub mod conflict_view;
#[cfg(feature = "gui")]
pub mod editor;
pub mod history_state;
#[cfg(feature = "gui")]
pub mod history_view;
pub mod i18n;
pub mod result_draft;
pub mod settings;
pub mod state;
pub mod text;
#[cfg(feature = "gui")]
pub mod theme;
pub mod worker;
