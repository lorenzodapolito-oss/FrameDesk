//! FrameDesk: analisi della qualità dei video.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod analyzer;
pub mod anchors;
pub mod app;
pub mod cache;
pub mod config;
pub mod ffmpeg;
pub mod imgproc;
pub mod metrics;
pub mod model;
pub mod report;
pub mod theme;
