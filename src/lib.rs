//! HourForge: a private, automatic time tracker for Windows.
//!
//! The logic lives in this library so that it can be unit-tested; `main.rs`
//! only parses the command line.
pub mod aggregate;
pub mod demo;
pub mod digest;
pub mod domain;
pub mod edit;
pub mod engine;
pub mod export;
pub mod icons;
pub mod model;
pub mod rules;
pub mod selftest;
pub mod sessions;
pub mod settings;
pub mod spans;
pub mod store;
pub mod sys;
pub mod timeutil;
pub mod tracker;
pub mod ui;
pub mod uia;
pub mod winfo;
