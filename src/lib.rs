//! Core library for Gitalyzer — an AI-powered terminal tool that analyzes Git
//! commit message quality and helps developers write better commits.
//!
//! Modules mirror the RFCs under `docs/rfcs/`:
//!
//! | Module       | RFC  | Responsibility                                    |
//! |--------------|------|----------------------------------------------------|
//! | [`cli`]      | 0001 | command-line surface                               |
//! | [`config`]   | 0002 | layered configuration and validation               |
//! | [`provider`] | 0003 | LLM adapters with schema-enforced JSON             |
//! | [`git`]      | 0004 | repository reads: history walk, staged extraction  |
//!
//! Further modules (analyze, write, output) land slice by slice per RFC 0008.

pub mod cli;
pub mod config;
pub mod git;
pub mod provider;
