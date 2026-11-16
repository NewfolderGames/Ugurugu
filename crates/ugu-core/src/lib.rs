// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Nyabi (nyabi-gh)

//! Document model: the layer tree and its validation, and the layer
//! operations whose meaning docs/rust/adr-operations-and-format.md fixes,
//! pinned down by tests with a reference evaluator.

pub mod brush;
pub mod clip;
pub mod command;
pub mod dither;
pub mod document;
pub mod edit;
pub mod gradient;
pub mod history;
pub mod keys;
pub mod motion;
pub mod ops;
pub mod pattern;
pub mod restyle;
pub mod selection;
pub mod store;
pub mod text;

#[cfg(test)]
mod semantics;
