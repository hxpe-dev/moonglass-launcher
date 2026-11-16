//! Platform-independent core of Moonglass.
//!
//! Nothing in this crate touches the OS: paths arrive already normalized by
//! the platform layer, time is passed in explicitly, and the launch state
//! machine is driven by observations rather than polling on its own. That
//! keeps every rule here unit-testable on any machine.

pub mod id;
pub mod library;
pub mod lifecycle;
pub mod template;
pub mod text;

pub use id::GameId;
pub use library::{Filter, Game, Source};
