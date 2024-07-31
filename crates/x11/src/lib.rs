//! X11 backend for srdwm. See [`platform`]'s module doc comment for the
//! actual implementation and how it compares to the legacy C++ backend.

mod platform;

pub use platform::X11Platform;
