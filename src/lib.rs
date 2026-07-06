//! Core library for Gitalyzer — an AI-powered terminal tool that analyzes Git
//! commit message quality and helps developers write better commits.
//!
//! Modules mirror the RFCs under `docs/rfcs/`:
//!
//! | Module     | RFC  | Responsibility                          |
//! |------------|------|------------------------------------------|
//! | [`cli`]    | 0001 | command-line surface                     |
//! | [`config`] | 0002 | layered configuration and validation     |
//!
//! Further modules (providers, git access, analyze, write, output) land slice by
//! slice per RFC 0008.

pub mod cli;
pub mod config;
