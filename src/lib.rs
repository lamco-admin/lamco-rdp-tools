//! lamco-rdp-tools: shared in-process library for the `rdpsee` and `rdpdo` binaries.
//!
//! The building-block modules (connection, session, gfx, capture, matching, ...)
//! are compiled into both tools. `rdpsee` observes a server; `rdpdo` drives an
//! authenticated session. Each tool is a thin binary that calls its app module's
//! `run_cli()`.

// Some report/session methods are part of the planned API but unused by one or
// both binaries today.
#![allow(dead_code)]

mod audio;
mod calibrate;
mod capture;
mod cert;
mod cli;
mod clipboard;
mod connection;
mod coords;
mod fingerprint;
mod gfx;
mod help;
mod input;
mod matching;
mod needle;
mod pixel;
mod probe;
mod provision;
mod recorder;
mod report;
mod script;
mod session;

pub mod rdpdo;
pub mod rdpsee;
