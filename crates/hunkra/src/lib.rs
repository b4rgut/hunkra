//! Application-local services; core domain models live in hunkra-core.
#[cfg(feature = "gui")]
pub mod editor;
pub mod i18n;
pub mod settings;
pub mod state;
pub mod text;
#[cfg(feature = "gui")]
pub mod theme;
pub mod worker;
