//! `native-browser-access.ts`: who controls a bot's browser, and the epochs that fence stale results.
//!
//! One host-owned controller and one privacy epoch per bot. Ownership is never derived from a client's parameters.
//! Every change of controller, page or privacy state moves an epoch, and anything captured under an older epoch
//! is refused when it comes back.

use protocol::browser::BrowserControl;

/// Why access was refused. The messages are the TypeScript ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError {
    /// Another device holds control.
    OtherDevice,
    /// The controller or the page changed since this caller looked.
    ControlChanged,
    /// A private interaction began, or a result came back under an older epoch.
    Discarded,
    /// The person has the browser; the bot waits.
    PersonControlling,
    /// The private interaction is not the one this caller began.
    PrivateChanged,
}

impl std::fmt::Display for AccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OtherDevice => "Another device is controlling this browser.",
            Self::ControlChanged => {
                "Browser control changed. Take control again and use the current frame."
            }
            Self::Discarded => "The browser interaction changed; this result was discarded.",
            Self::PersonControlling => {
                "The person is controlling this browser. Wait until they return control."
            }
            Self::PrivateChanged => "The private interaction changed.",
        })
    }
}

impl std::error::Error for AccessError {}

/// The epochs at the moment something started; compared again when its result arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    pub control_epoch: u64,
    pub private_epoch: u64,
}

#[derive(Debug, Default)]
pub struct Access {
    owner: Option<String>,
    control_epoch: u64,
    private_epoch: u64,
    private_active: bool,
}

impl Access {
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    pub fn control_epoch(&self) -> u64 {
        self.control_epoch
    }

    pub fn private_epoch(&self) -> u64 {
        self.private_epoch
    }

    pub fn private_active(&self) -> bool {
        self.private_active
    }

    /// The person has the browser while someone controls it, and always during a private interaction.
    pub fn control(&self) -> BrowserControl {
        if self.owner.is_some() || self.private_active {
            BrowserControl::Human
        } else {
            BrowserControl::Bot
        }
    }

    pub fn take(&mut self, owner: &str) -> Result<(), AccessError> {
        match self.owner.as_deref() {
            Some(current) if current != owner => Err(AccessError::OtherDevice),
            Some(_) => Ok(()),
            None => {
                self.owner = Some(owner.to_string());
                self.control_epoch += 1;
                Ok(())
            }
        }
    }

    /// Hands the browser back to the bot. `false` if `owner` did not hold it.
    pub fn release(&mut self, owner: &str) -> bool {
        if self.owner.as_deref() != Some(owner) {
            return false;
        }
        self.owner = None;
        self.control_epoch += 1;
        true
    }

    pub fn require(&self, owner: &str, epoch: u64) -> Result<(), AccessError> {
        if self.owner.as_deref() == Some(owner) && self.control_epoch == epoch {
            Ok(())
        } else {
            Err(AccessError::ControlChanged)
        }
    }

    /// The page changed under whoever controls it: what they were looking at is stale.
    pub fn changed_page(&mut self) {
        self.control_epoch += 1;
    }

    pub fn ticket(&self) -> Ticket {
        Ticket {
            control_epoch: self.control_epoch,
            private_epoch: self.private_epoch,
        }
    }

    /// `automation` also requires that the bot, not a person, holds the browser.
    pub fn check(&self, ticket: Ticket, automation: bool) -> Result<(), AccessError> {
        if self.private_active
            || ticket.private_epoch != self.private_epoch
            || ticket.control_epoch != self.control_epoch
        {
            return Err(AccessError::Discarded);
        }
        if automation && self.control() != BrowserControl::Bot {
            return Err(AccessError::PersonControlling);
        }
        Ok(())
    }

    pub fn begin_private(&mut self, owner: &str, epoch: u64) -> Result<u64, AccessError> {
        self.require(owner, epoch)?;
        self.private_active = true;
        self.private_epoch += 1;
        self.control_epoch += 1;
        Ok(self.private_epoch)
    }

    pub fn end_private(&mut self, owner: &str, epoch: u64) -> Result<(), AccessError> {
        if !self.private_active
            || self.owner.as_deref() != Some(owner)
            || self.private_epoch != epoch
        {
            return Err(AccessError::PrivateChanged);
        }
        self.private_active = false;
        self.private_epoch += 1;
        self.control_epoch += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_second_controller_and_old_input_across_takeover_and_page_changes() {
        let mut access = Access::default();
        access.take("device-a").unwrap();
        let first = access.control_epoch();
        assert_eq!(access.take("device-b"), Err(AccessError::OtherDevice));
        assert_eq!(
            access.require("device-b", first),
            Err(AccessError::ControlChanged)
        );
        access.changed_page();
        assert_eq!(
            access.require("device-a", first),
            Err(AccessError::ControlChanged)
        );
        assert!(!access.release("device-b"));
        assert!(access.release("device-a"));
        access.take("device-b").unwrap();
        assert!(access.require("device-a", first).is_err());
    }

    #[test]
    fn fences_queued_and_late_results_and_stays_private_after_disconnect_or_hiding() {
        let mut access = Access::default();
        let before = access.ticket();
        access.take("device-a").unwrap();
        assert!(access.check(before, true).is_err());
        let capture = access.ticket();
        let epoch = access
            .begin_private("device-a", access.control_epoch())
            .unwrap();
        assert!(access.check(capture, false).is_err());
        access.release("device-a");
        assert_eq!(access.control(), BrowserControl::Human);
        assert!(access.private_active());
        assert!(access.check(access.ticket(), true).is_err());
        assert_eq!(
            access.end_private("device-b", epoch),
            Err(AccessError::PrivateChanged)
        );
        access.take("device-b").unwrap();
        access.end_private("device-b", epoch).unwrap();
        assert!(access.check(capture, false).is_err());
        assert_eq!(access.control(), BrowserControl::Human);
        access.release("device-b");
        assert_eq!(access.control(), BrowserControl::Bot);
    }

    #[test]
    fn automation_waits_for_the_person_and_a_matching_ticket_passes() {
        let mut access = Access::default();
        assert!(access.check(access.ticket(), true).is_ok());
        let ticket = access.ticket();
        access.take("local").unwrap();
        assert_eq!(
            access.check(access.ticket(), true),
            Err(AccessError::PersonControlling)
        );
        assert_eq!(access.check(ticket, false), Err(AccessError::Discarded));
        assert!(access.check(access.ticket(), false).is_ok());
    }
}
