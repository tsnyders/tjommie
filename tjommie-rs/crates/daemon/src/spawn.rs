//! The one place the daemon creates processes. Every child gets `CREATE_NO_WINDOW`, so no console window ever
//! opens. `clippy.toml` bans `Command::new` everywhere else.

use std::ffi::OsStr;
use std::ops::{Deref, DerefMut};
use std::os::windows::process::CommandExt;
use std::process::Command;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `std::process::Command` that was created with `CREATE_NO_WINDOW`. The flags are kept so tests can read them
/// back; std has no getter for them.
pub struct NoWindowCommand {
    command: Command,
    flags: u32,
}

pub fn command(program: impl AsRef<OsStr>) -> NoWindowCommand {
    #[allow(clippy::disallowed_methods)] // The one allowed call site.
    let command = Command::new(program);
    let mut command = NoWindowCommand { command, flags: 0 };
    command.creation_flags(0);
    command
}

impl NoWindowCommand {
    pub fn flags(&self) -> u32 {
        self.flags
    }

    /// Sets process creation flags. `CREATE_NO_WINDOW` always stays set. This method, not std's
    /// `CommandExt::creation_flags` (which would replace it), is the way to add flags; `clippy.toml` bans std's.
    pub fn creation_flags(&mut self, flags: u32) -> &mut Self {
        self.flags = CREATE_NO_WINDOW | flags;
        #[allow(clippy::disallowed_methods)] // The one place creation flags reach std.
        self.command.creation_flags(self.flags);
        self
    }
}

/// For `tokio::process::Command::from`. The flags stay set on the command.
impl From<NoWindowCommand> for Command {
    fn from(command: NoWindowCommand) -> Command {
        command.command
    }
}

impl Deref for NoWindowCommand {
    type Target = Command;
    fn deref(&self) -> &Command {
        &self.command
    }
}

impl DerefMut for NoWindowCommand {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_sets_create_no_window() {
        let command = command("x");
        assert_eq!(command.flags(), 0x0800_0000);
        assert_eq!(command.get_program(), "x");
    }

    #[test]
    fn added_creation_flags_keep_create_no_window() {
        let mut command = command("x");
        command.creation_flags(0x200);
        assert_eq!(command.flags(), 0x0800_0200);
    }
}
