//! Taskbar and desktop weather domain: the view-independent data layer, the shared
//! presentation rules and the shell-published forecast feed decoder.
//!
//! The shell owns fetching, refresh and settings; this crate holds no network, timer or UI code.

pub mod weather;
pub mod weather_feed;
pub mod weather_tint;
pub mod weather_view;
