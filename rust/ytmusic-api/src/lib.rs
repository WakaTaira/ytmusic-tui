//! YouTube Music InnerTube client — auth + transport core.
//!
//! This crate reimplements the protocol layer that the Python `ytmusic-tui`
//! delegates to `ytmusicapi`: browser-header authentication (the per-request
//! `SAPISIDHASH` signature), the `youtubei/v1` request shape, and the session
//! canary that detects YouTube's "valid-looking but logged-out" HTTP 200
//! responses.
//!
//! # Authentication
//!
//! Auth material is a ytmusicapi-format `browser.json`: a flat JSON map of raw
//! HTTP headers including `Cookie` (which must carry a SAPISID value) and
//! `Authorization`. Load it with [`BrowserAuth::load`], then build an
//! [`InnerTubeClient`].
//!
//! # The canary
//!
//! Auth expiry does **not** surface as an HTTP error — YouTube serves
//! logged-out pages with HTTP 200. [`InnerTubeClient::is_session_valid`] hits
//! the account endpoint and treats a missing signed-in structure as
//! "logged out", returning `false` rather than erroring.

mod auth;
mod client;
mod context;
mod endpoints;
mod error;
pub mod models;
mod nav;
pub mod parse;

pub use auth::{BrowserAuth, YTM_ORIGIN, sapisid_authorization};
pub use client::{AccountInfo, InnerTubeClient};
pub use context::{CLIENT_NAME, build_context};
pub use error::{ApiError, AuthLoadError};

// Re-export domain types at the crate root for ergonomic imports.
pub use models::{
    AlbumInfo, ArtistInfo, HomeSection, HomeSectionItem, PlaylistInfo, RelatedArtist,
    SearchResults, Track,
};
