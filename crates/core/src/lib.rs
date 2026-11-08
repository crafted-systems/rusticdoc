//! rusticdoc core: the document pipeline without any HTTP.
//!
//! A request carries a template (or a plain document), JSON data and an
//! ordered list of steps. The pipeline fills the template, then runs the
//! steps: conversion with LibreOffice, then PDF operations with the
//! configured engines. The CLI and the server both drive this crate.

pub mod config;
pub mod convert;
pub mod error;
pub mod fonts;
pub mod job;
pub mod limits;
pub mod pdfform;
pub mod pdfops;
pub mod pipeline;
pub mod process;
pub mod render;
pub mod store;
pub mod template;

pub use config::Config;
pub use error::{Error, Result};
pub use job::{Document, Format, Output, RenderRequest, Step};
pub use pipeline::{Engines, Pipeline};

/// Version of the engine, from the workspace manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
