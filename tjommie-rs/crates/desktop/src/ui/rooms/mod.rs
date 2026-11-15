//! Rooms (Phase 3 plan T5.2; `app/room`, `room/*`): the rooms list, a room (its posts, composer and members panel),
//! and the room form.

pub mod composer;
pub mod form;
pub mod list;
mod members;
pub mod post;
pub mod screen;

pub use form::open_room_form;

#[cfg(test)]
mod tests;
