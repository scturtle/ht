//! Shared input events, sequence decoding and key lifecycle tracking.
//! Host terminal I/O and shell bindings live with their client owners.

mod lease;
mod model;
pub(crate) mod mouse;
mod parse;

pub(crate) use lease::{InputLease, InputLeaseKey, InputLeaseTable, RepeatPlan};
pub use model::{KeyIdentity, KeyboardProtocol, TerminalKey, TextCommit, WindowsKeyRecord};
pub use parse::parse_terminal_key_sequence;
