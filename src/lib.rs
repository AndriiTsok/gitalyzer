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
//! | [`analyze`]  | 0005 | batching, critique pipeline, deterministic stats   |
//! | [`write`]    | 0006 | staged-context budgeting, suggestion task          |
//! | [`output`]   | 0007 | human and JSON report rendering                    |

pub mod analyze;
pub mod cli;
pub mod config;
pub mod git;
pub mod output;
pub mod provider;
pub mod write;
